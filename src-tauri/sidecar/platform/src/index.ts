#!/usr/bin/env node
import * as fs from 'node:fs';
import * as http from 'node:http';
import * as path from 'node:path';
import * as lark from '@larksuiteoapi/node-sdk';
import { QQBotClient, type QQMessage } from './qq-bot';

type Channel = {
  id: string;
  platform: 'feishu' | 'qq';
  appId: string;
  appSecret: string;
  label?: string;
  enabled?: boolean;
  assignment?: 'frogclaw' | 'none';
  sandbox?: boolean;
};

type AgentEvent =
  | { type: 'system'; model?: string }
  | { type: 'text'; delta: string }
  | { type: 'tool_use'; name: string; detail: string }
  | { type: 'tool_result' }
  | { type: 'result'; error?: string; totalTokens?: number; durationMs?: number; model?: string };

type Bot = {
  channel: Channel;
  client: lark.Client;
  ws: lark.WSClient | null;
  botOpenId: string | null;
  status: 'starting' | 'running' | 'error' | 'stopped';
  error: string | null;
  recentPrivateUsers: Set<string>;
  activeChats: Set<string>;
};

type QQBot = {
  channel: Channel;
  client: QQBotClient;
  status: 'starting' | 'running' | 'error' | 'stopped';
  error: string | null;
  activeSessions: Set<string>;
};

const args = parseArgs(process.argv);
const home = process.env.USERPROFILE || process.env.HOME || '';
const configDir = path.join(home, '.frogclaw');
const logPath = path.join(configDir, 'platform-sidecar.log');
const knownUsersDir = path.join(configDir, 'feishu-recent-users');
const mediaDir = path.join(process.env.TEMP || process.env.TMPDIR || configDir, 'frogclaw-platform-media');
fs.mkdirSync(configDir, { recursive: true });
fs.mkdirSync(mediaDir, { recursive: true });

let parentBaseUrl = args.parent || '';
const startedAt = Date.now();
const bots = new Map<string, Bot>();
const qqBots = new Map<string, QQBot>();
const sseClients = new Set<http.ServerResponse>();

function parseArgs(argv: string[]) {
  const out = { port: 18788, parent: '' };
  for (let i = 2; i < argv.length; i++) {
    if (argv[i] === '--port') out.port = Number(argv[++i]) || 18788;
    if (argv[i] === '--parent') out.parent = argv[++i] || '';
  }
  return out;
}

function log(level: string, ...parts: unknown[]) {
  const line = `[sidecar ${level}] ${parts.map((p) => typeof p === 'string' ? p : JSON.stringify(p)).join(' ')}\n`;
  try { process.stderr.write(line); } catch {}
  try { fs.appendFileSync(logPath, line); } catch {}
}

function channelsPath() {
  return path.join(configDir, 'im-channels.json');
}

function loadChannels(): Channel[] {
  try {
    const p = channelsPath();
    if (!fs.existsSync(p)) return [];
    const parsed = JSON.parse(fs.readFileSync(p, 'utf8'));
    const channels = Array.isArray(parsed.channels) ? parsed.channels : [];
    return channels
      .filter((ch: any) => ch?.platform === 'feishu' || ch?.platform === 'qq')
      .map((ch: any) => ({
        id: ch.id || `${ch.platform}-${ch.appId || Date.now()}`,
        platform: ch.platform === 'qq' ? 'qq' : 'feishu',
        appId: ch.appId || ch.app_id || '',
        appSecret: ch.appSecret || ch.app_secret || '',
        label: ch.label || '',
        enabled: ch.enabled !== false,
        assignment: ch.assignment || 'frogclaw',
        sandbox: Boolean(ch.sandbox),
      }))
      .filter((ch: Channel) => ch.appId && ch.appSecret);
  } catch (e: any) {
    log('warn', 'loadChannels failed:', e.message);
    return [];
  }
}

function knownUsersPath(appId: string) {
  return path.join(knownUsersDir, `${appId}.json`);
}

function loadKnownUsers(appId: string): Set<string> {
  try {
    const p = knownUsersPath(appId);
    if (!fs.existsSync(p)) return new Set();
    const parsed = JSON.parse(fs.readFileSync(p, 'utf8'));
    return new Set(Array.isArray(parsed) ? parsed.filter((v) => typeof v === 'string') : []);
  } catch {
    return new Set();
  }
}

function saveKnownUsers(appId: string, users: Set<string>) {
  try {
    fs.mkdirSync(knownUsersDir, { recursive: true });
    fs.writeFileSync(knownUsersPath(appId), JSON.stringify([...users]));
  } catch {}
}

function notifyStatus() {
  const payload = JSON.stringify({
    type: 'status',
    uptimeMs: Date.now() - startedAt,
    bots: [
      ...[...bots.values()].map((bot) => ({
        appId: bot.channel.appId,
        platform: 'feishu',
        label: bot.channel.label || '',
        assignment: bot.channel.assignment || 'frogclaw',
        status: bot.status,
        error: bot.error,
        agent: bot.channel.assignment === 'none' ? null : 'frogclaw',
      })),
      ...[...qqBots.values()].map((bot) => ({
        appId: bot.channel.appId,
        platform: 'qq',
        label: bot.channel.label || '',
        assignment: bot.channel.assignment || 'frogclaw',
        status: bot.status,
        error: bot.error,
        agent: bot.channel.assignment === 'none' ? null : 'frogclaw',
      })),
    ],
  });
  for (const res of sseClients) {
    try { res.write(`data: ${payload}\n\n`); } catch {}
  }
}

function buildNotificationCard(title: string, color: string, body: string) {
  return JSON.stringify({
    config: { wide_screen_mode: true },
    header: { template: color, title: { content: title, tag: 'plain_text' } },
    elements: [{ tag: 'markdown', content: body }],
  });
}

function buildResultCard(state: {
  status: 'thinking' | 'running' | 'complete' | 'error';
  text: string;
  error?: string;
  model?: string;
  totalTokens?: number;
  durationMs?: number;
}) {
  const status = {
    thinking: { color: 'blue', title: 'Thinking...' },
    running: { color: 'blue', title: 'Running...' },
    complete: { color: 'green', title: 'Complete' },
    error: { color: 'red', title: 'Error' },
  }[state.status];
  const elements: any[] = [];
  if (state.text) elements.push({ tag: 'markdown', content: truncate(state.text) });
  if (!state.text && state.status === 'thinking') elements.push({ tag: 'markdown', content: '_FrogClawClient is thinking..._' });
  if (state.error) elements.push({ tag: 'markdown', content: `**Error:** ${state.error}` });
  const foot: string[] = [];
  if (state.model) foot.push(state.model);
  if (state.totalTokens) foot.push(`${state.totalTokens} tokens`);
  if (state.durationMs) foot.push(`${(state.durationMs / 1000).toFixed(1)}s`);
  if (foot.length) elements.push({ tag: 'note', elements: [{ tag: 'plain_text', content: foot.join(' | ') }] });
  return JSON.stringify({
    config: { wide_screen_mode: true },
    header: { template: status.color, title: { content: status.title, tag: 'plain_text' } },
    elements,
  });
}

function truncate(text: string) {
  if (text.length <= 28000) return text;
  return `${text.slice(0, 14000)}\n\n... (truncated) ...\n\n${text.slice(-14000)}`;
}

async function sendText(client: lark.Client, chatId: string, text: string, userId?: string) {
  const content = JSON.stringify({ text });
  try {
    await client.im.v1.message.create({
      params: { receive_id_type: 'chat_id' },
      data: { receive_id: chatId, msg_type: 'text', content },
    });
    return;
  } catch {}
  if (userId) {
    await client.im.v1.message.create({
      params: { receive_id_type: 'open_id' },
      data: { receive_id: userId, msg_type: 'text', content },
    }).catch(() => {});
  }
}

async function sendCard(client: lark.Client, chatId: string, content: string, replyToMessageId?: string, userId?: string): Promise<string | null> {
  if (replyToMessageId) {
    try {
      const resp = await (client as any).im.v1.message.reply({
        path: { message_id: replyToMessageId },
        data: { msg_type: 'interactive', content, reply_in_thread: false },
      });
      if (resp?.data?.message_id) return resp.data.message_id;
    } catch {}
  }
  try {
    const resp = await client.im.v1.message.create({
      params: { receive_id_type: 'chat_id' },
      data: { receive_id: chatId, msg_type: 'interactive', content },
    });
    if ((resp as any)?.data?.message_id) return (resp as any).data.message_id;
  } catch {}
  if (userId) {
    try {
      const resp = await client.im.v1.message.create({
        params: { receive_id_type: 'open_id' },
        data: { receive_id: userId, msg_type: 'interactive', content },
      });
      if ((resp as any)?.data?.message_id) return (resp as any).data.message_id;
    } catch {}
  }
  return null;
}

async function patchCard(client: lark.Client, messageId: string, content: string) {
  await client.im.v1.message.patch({ path: { message_id: messageId }, data: { content } }).catch((e: any) => {
    log('warn', 'patchCard failed:', e.message);
  });
}

async function addTypingReaction(client: lark.Client, messageId: string) {
  try {
    const resp = await (client as any).im.v1.messageReaction.create({
      path: { message_id: messageId },
      data: { reaction_type: { emoji_type: 'THUMBSUP' } },
    });
    return resp?.data?.reaction_id as string | undefined;
  } catch {
    return undefined;
  }
}

async function removeTypingReaction(client: lark.Client, messageId: string, reactionId?: string) {
  if (!reactionId) return;
  await (client as any).im.v1.messageReaction.delete({
    path: { message_id: messageId, reaction_id: reactionId },
  }).catch(() => {});
}

async function downloadResource(client: lark.Client, messageId: string, fileKey: string, name: string, type: string) {
  try {
    const resp = await (client as any).im.v1.messageResource.get({
      path: { message_id: messageId, file_key: fileKey },
      params: { type },
    });
    const target = path.join(mediaDir, `${Date.now()}-${name}`);
    await resp.writeFile(target);
    return target;
  } catch (e: any) {
    log('warn', 'downloadResource failed:', e.message);
    return null;
  }
}

function extractPostText(content: any): string {
  const blocks = Array.isArray(content?.content) ? [content] : Object.values(content || {});
  const lines: string[] = [];
  for (const block of blocks as any[]) {
    if (typeof block?.title === 'string') lines.push(block.title);
    for (const paragraph of block?.content || []) {
      const line = (paragraph || [])
        .map((item: any) => item?.text || '')
        .join('');
      if (line) lines.push(line);
    }
  }
  return lines.join('\n');
}

async function callParentStream(body: any, onEvent: (evt: AgentEvent) => Promise<void>) {
  if (!parentBaseUrl) throw new Error('parent bridge URL missing');
  const resp = await fetch(`${parentBaseUrl}/message`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!resp.ok || !resp.body) throw new Error(`parent bridge error: ${resp.status}`);
  const reader = resp.body.getReader();
  const decoder = new TextDecoder();
  let buf = '';
  while (true) {
    const { value, done } = await reader.read();
    if (done) break;
    buf += decoder.decode(value, { stream: true });
    let idx;
    while ((idx = buf.indexOf('\n\n')) >= 0) {
      const block = buf.slice(0, idx);
      buf = buf.slice(idx + 2);
      for (const line of block.split('\n')) {
        if (!line.startsWith('data:')) continue;
        const raw = line.slice(5).trim();
        if (!raw) continue;
        await onEvent(JSON.parse(raw));
      }
    }
  }
}

function createDispatcher(bot: Bot) {
  return new lark.EventDispatcher({}).register({
    'im.message.receive_v1': async (data: any) => {
      try {
        const event = data?.event || data;
        const msg = event?.message || {};
        const sender = event?.sender || {};
        const messageId = msg.message_id;
        const chatId = msg.chat_id;
        const chatType = msg.chat_type;
        const userId = sender?.sender_id?.open_id || sender?.sender_id?.user_id || '';
        if (!messageId || !chatId || !userId) return;

        let parsed: any = {};
        try { parsed = JSON.parse(msg.content || '{}'); } catch {}
        let text = '';
        if (msg.message_type === 'text') text = parsed.text || '';
        else if (msg.message_type === 'post') text = extractPostText(parsed);
        else text = parsed.text || parsed.file_name || '';
        text = text.replace(/<at[^>]*>.*?<\/at>/g, '').trim();

        const mentions = Array.isArray(msg.mentions) ? msg.mentions : [];
        const mentioned = mentions.some((m: any) => m?.id?.open_id === bot.botOpenId || m?.id?.user_id === bot.botOpenId);
        if (chatType !== 'p2p' && !mentioned) return;

        if (chatType === 'p2p') {
          bot.recentPrivateUsers.add(userId);
          saveKnownUsers(bot.channel.appId, bot.recentPrivateUsers);
        }

        if (bot.channel.assignment === 'none') {
          await sendText(bot.client, chatId, '该飞书机器人还未启用，请在 FrogClawClient 的 IM 通道中启用。', userId);
          return;
        }

        const sessionKey = `feishu:${bot.channel.appId}:${chatId}:${userId}`;
        const lower = text.toLowerCase();
        if (lower === '/new' || lower === '/reset') {
          await fetch(`${parentBaseUrl}/reset`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ sessionKey }) }).catch(() => {});
          await sendText(bot.client, chatId, '会话已重置，下一条消息会创建新的项目对话。', userId);
          return;
        }
        if (lower === '/status') {
          await sendText(bot.client, chatId, `FrogClawClient IM 已连接\nBot: ${bot.channel.label || bot.channel.appId.slice(0, 12)}\nStatus: ${bot.status}`, userId);
          return;
        }
        if (lower === '/stop') {
          await fetch(`${parentBaseUrl}/cancel`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ sessionKey }) }).catch(() => {});
          await sendText(bot.client, chatId, '已请求停止当前回复。', userId);
          return;
        }
        if (!text && msg.message_type !== 'image' && msg.message_type !== 'file') return;
        if (bot.activeChats.has(chatId)) {
          await sendText(bot.client, chatId, '当前会话正在回复，请等待完成后再发送新消息。', userId);
          return;
        }

        const files: string[] = [];
        if (msg.message_type === 'image' && parsed.image_key) {
          const p = await downloadResource(bot.client, messageId, parsed.image_key, `${parsed.image_key}.png`, 'image');
          if (p) files.push(p);
        }
        if (msg.message_type === 'file' && parsed.file_key) {
          const p = await downloadResource(bot.client, messageId, parsed.file_key, parsed.file_name || parsed.file_key, 'file');
          if (p) files.push(p);
        }

        bot.activeChats.add(chatId);
        const state = { status: 'thinking' as const, text: '', model: undefined as string | undefined };
        let cardId = await sendCard(bot.client, chatId, buildResultCard(state), messageId, userId);
        const reactionId = await addTypingReaction(bot.client, messageId);
        let lastPatch = 0;
        const patch = async (force = false) => {
          if (!cardId) return;
          const now = Date.now();
          if (!force && now - lastPatch < 350) return;
          lastPatch = now;
          await patchCard(bot.client, cardId, buildResultCard(state));
        };

        try {
          await callParentStream({ sessionKey, prompt: text || '请分析附件', files }, async (evt) => {
            if (evt.type === 'system') {
              state.status = 'running';
              state.model = evt.model || state.model;
            } else if (evt.type === 'text') {
              state.status = 'running';
              state.text += evt.delta;
            } else if (evt.type === 'result') {
              state.status = evt.error ? 'error' : 'complete';
              Object.assign(state, evt);
            }
            await patch(evt.type === 'result');
          });
        } catch (e: any) {
          state.status = 'error';
          (state as any).error = e.message || String(e);
          await patch(true);
        } finally {
          await removeTypingReaction(bot.client, messageId, reactionId);
          bot.activeChats.delete(chatId);
        }
      } catch (e: any) {
        log('error', 'message handler:', e.message);
      }
    },
  });
}

async function connectBot(channel: Channel) {
  const client = new lark.Client({ appId: channel.appId, appSecret: channel.appSecret, disableTokenCache: false });
  const bot: Bot = {
    channel,
    client,
    ws: null,
    botOpenId: null,
    status: 'starting',
    error: null,
    recentPrivateUsers: loadKnownUsers(channel.appId),
    activeChats: new Set(),
  };
  bots.set(channel.appId, bot);
  try {
    try {
      const info: any = await (client as any).bot?.v3?.botInfo?.get?.() || await client.request({ method: 'GET', url: 'https://open.feishu.cn/open-apis/bot/v3/info' });
      bot.botOpenId = info?.data?.bot?.open_id || info?.bot?.open_id || null;
    } catch (e: any) {
      log('warn', 'bot info failed:', e.message);
    }
    const ws = new lark.WSClient({ appId: channel.appId, appSecret: channel.appSecret, loggerLevel: lark.LoggerLevel.info });
    await ws.start({ eventDispatcher: createDispatcher(bot) });
    bot.ws = ws;
    bot.status = 'running';
    const card = buildNotificationCard('已连接 FrogClawClient', 'green', `飞书机器人已上线${channel.label ? ` (${channel.label})` : ''}\n\n**后端:** FrogClawClient 对话`);
    for (const userId of bot.recentPrivateUsers) {
      await client.im.v1.message.create({
        params: { receive_id_type: 'open_id' },
        data: { receive_id: userId, msg_type: 'interactive', content: card },
      }).catch(() => {});
    }
  } catch (e: any) {
    bot.status = 'error';
    bot.error = e.message || String(e);
    log('error', 'connectBot failed:', bot.error);
  }
  notifyStatus();
}

async function handleQQMessage(bot: QQBot, msg: QQMessage) {
  const sessionKey = `qq:${bot.channel.appId}:${msg.replyCtx.messageType}:${msg.replyCtx.groupOpenId || msg.replyCtx.userOpenId}`;
  const text = (msg.text || '').trim();
  const lower = text.toLowerCase();

  if (bot.channel.assignment === 'none') {
    await bot.client.sendText(msg.replyCtx, '该 QQ 机器人还未启用，请在 FrogClawClient 的 IM 通道中分配后端。');
    return;
  }
  if (lower === '/new' || lower === '/reset') {
    await fetch(`${parentBaseUrl}/reset`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ sessionKey }),
    }).catch(() => {});
    await bot.client.sendText(msg.replyCtx, '会话已重置，下一条消息会创建新的项目对话。');
    return;
  }
  if (lower === '/status') {
    await bot.client.sendText(msg.replyCtx, `FrogClawClient IM 已连接\nBot: ${bot.channel.label || bot.channel.appId}\nStatus: ${bot.status}`);
    return;
  }
  if (lower === '/stop') {
    await fetch(`${parentBaseUrl}/cancel`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ sessionKey }),
    }).catch(() => {});
    await bot.client.sendText(msg.replyCtx, '已请求停止当前回复。');
    return;
  }
  if (!text && msg.imagePaths.length === 0) return;
  if (bot.activeSessions.has(sessionKey)) {
    await bot.client.sendText(msg.replyCtx, '当前会话正在回复，请等待完成后再发送新消息。');
    return;
  }

  bot.activeSessions.add(sessionKey);
  await bot.client.sendText(msg.replyCtx, 'FrogClawClient 正在思考...');
  let answer = '';
  let model = '';
  let error = '';
  let totalTokens: number | undefined;
  let durationMs: number | undefined;
  try {
    await callParentStream(
      {
        sessionKey,
        prompt: text || '请分析附件',
        files: msg.imagePaths,
      },
      async (evt) => {
        if (evt.type === 'system') {
          model = evt.model || model;
        } else if (evt.type === 'text') {
          answer += evt.delta;
        } else if (evt.type === 'result') {
          error = evt.error || '';
          model = evt.model || model;
          totalTokens = evt.totalTokens;
          durationMs = evt.durationMs;
        }
      },
    );
  } catch (e: any) {
    error = e.message || String(e);
  } finally {
    bot.activeSessions.delete(sessionKey);
  }

  if (error) {
    await bot.client.sendText(msg.replyCtx, `错误：${error}`);
    return;
  }

  const foot: string[] = [];
  if (model) foot.push(model);
  if (totalTokens) foot.push(`${totalTokens} tokens`);
  if (durationMs) foot.push(`${(durationMs / 1000).toFixed(1)}s`);
  await bot.client.sendText(msg.replyCtx, `${answer || '已完成。'}${foot.length ? `\n\n${foot.join(' | ')}` : ''}`);
}

async function connectQQBot(channel: Channel) {
  const client = new QQBotClient({
    appId: channel.appId,
    appSecret: channel.appSecret,
    sandbox: channel.sandbox,
  });
  const bot: QQBot = {
    channel,
    client,
    status: 'starting',
    error: null,
    activeSessions: new Set(),
  };
  qqBots.set(channel.appId, bot);
  client.on('message', (msg: QQMessage) => {
    handleQQMessage(bot, msg).catch((e: any) => log('error', 'qq message handler:', e.message || String(e)));
  });
  client.on('error', (e: Error) => {
    bot.status = 'error';
    bot.error = e.message || String(e);
    notifyStatus();
  });
  client.on('status', (status: string) => {
    bot.status = status === 'running' ? 'running' : 'starting';
    notifyStatus();
  });

  try {
    await client.start();
    bot.status = 'running';
    bot.error = null;
    log('info', `QQ bot connected: ${channel.appId}`);
  } catch (e: any) {
    bot.status = 'error';
    bot.error = e.message || String(e);
    log('error', 'connectQQBot failed:', bot.error);
  }
  notifyStatus();
}

async function disconnectAll() {
  for (const bot of bots.values()) {
    try { await bot.ws?.shutdown?.(); } catch {}
    try { await bot.ws?.stop?.(); } catch {}
    bot.status = 'stopped';
  }
  bots.clear();
  for (const bot of qqBots.values()) {
    try { await bot.client.stop(); } catch {}
    bot.status = 'stopped';
  }
  qqBots.clear();
  notifyStatus();
}

async function reconcile() {
  await disconnectAll();
  for (const channel of loadChannels()) {
    if (channel.enabled === false) continue;
    if (channel.platform === 'qq') await connectQQBot(channel);
    else await connectBot(channel);
  }
  return [
    ...[...bots.values()].filter((b) => b.status === 'running'),
    ...[...qqBots.values()].filter((b) => b.status === 'running'),
  ].length;
}

function sendJson(res: http.ServerResponse, status: number, obj: any) {
  const body = JSON.stringify(obj);
  res.writeHead(status, { 'content-type': 'application/json; charset=utf-8', 'content-length': Buffer.byteLength(body) });
  res.end(body);
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url || '/', 'http://127.0.0.1');
  if (req.method === 'GET' && url.pathname === '/health') {
    return sendJson(res, 200, {
      ok: true,
      uptimeMs: Date.now() - startedAt,
      bots: [
        ...[...bots.values()].map((b) => ({ appId: b.channel.appId, platform: 'feishu', status: b.status, error: b.error })),
        ...[...qqBots.values()].map((b) => ({ appId: b.channel.appId, platform: 'qq', status: b.status, error: b.error })),
      ],
    });
  }
  if (req.method === 'GET' && url.pathname === '/events') {
    res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache', connection: 'keep-alive' });
    res.write(': connected\n\n');
    sseClients.add(res);
    notifyStatus();
    req.on('close', () => sseClients.delete(res));
    return;
  }
  if (req.method === 'POST' && (url.pathname === '/connect' || url.pathname === '/reload')) {
    try {
      const count = await reconcile();
      return sendJson(res, 200, { ok: true, botsConnected: count });
    } catch (e: any) {
      return sendJson(res, 500, { ok: false, error: e.message });
    }
  }
  if (req.method === 'POST' && url.pathname === '/disconnect') {
    await disconnectAll();
    return sendJson(res, 200, { ok: true });
  }
  sendJson(res, 404, { ok: false, error: 'not found' });
});

server.on('error', (error: NodeJS.ErrnoException) => {
  if (error.code === 'EADDRINUSE') {
    log('warn', `127.0.0.1:${args.port} is already in use; another FrogClaw platform sidecar may already be running.`);
    process.exit(0);
  }
  log('error', 'server error:', error.message || String(error));
  process.exit(1);
});

server.listen(args.port, '127.0.0.1', () => {
  const addr = server.address();
  const port = typeof addr === 'object' && addr ? addr.port : args.port;
  process.stdout.write(`FROGCLAW_PLATFORM_READY port=${port}\n`);
  log('info', `listening on 127.0.0.1:${port}, parent=${parentBaseUrl}`);
});

process.on('SIGTERM', () => disconnectAll().finally(() => process.exit(0)));
process.on('SIGINT', () => disconnectAll().finally(() => process.exit(0)));
