/**
 * QQ Bot API v2 Client — Official QQ Bot API integration.
 *
 * Connects to the QQ Open Platform via OAuth2 + WebSocket gateway.
 * Receives GROUP_AT_MESSAGE_CREATE and C2C_MESSAGE_CREATE events.
 * Sends replies via REST API.
 *
 * Reference: cc-connect/platform/qqbot/qqbot.go
 */

import { EventEmitter } from 'node:events';
import * as https from 'node:https';
import WebSocket from 'ws';

// ============================================================================
// Types
// ============================================================================

export interface QQBotConfig {
  appId: string;
  appSecret: string;
  sandbox?: boolean;
}

export interface QQReplyContext {
  messageType: 'group' | 'c2c';
  groupOpenId?: string;
  userOpenId: string;
  eventMsgId: string;
}

export interface QQMessage {
  id: string;
  text: string;
  replyCtx: QQReplyContext;
  senderName: string;
  /** Local paths to downloaded image attachments. */
  imagePaths: string[];
}

// ============================================================================
// Constants
// ============================================================================

const TOKEN_URL = 'https://bots.qq.com/app/getAppAccessToken';
const API_BASE_PROD = 'https://api.sgroup.qq.com';
const API_BASE_SANDBOX = 'https://sandbox.api.sgroup.qq.com';

const OP_DISPATCH = 0;
const OP_HEARTBEAT = 1;
const OP_IDENTIFY = 2;
const OP_RESUME = 6;
const OP_RECONNECT = 7;
const OP_INVALID_SESSION = 9;
const OP_HELLO = 10;
const OP_HEARTBEAT_ACK = 11;

const INTENT_GROUP_AND_C2C = 1 << 25;

const MAX_RECONNECT_ATTEMPTS = 30;
const TOKEN_REFRESH_MARGIN_MS = 5 * 60 * 1000;
const MSG_SEQ_TTL_MS = 5 * 60 * 1000;
const DEFAULT_HEARTBEAT_MS = 41250;
const MAX_MSG_LENGTH = 2000;

// ============================================================================
// Logger (to stderr, matches sidecar pattern)
// ============================================================================

function log(level: string, ...parts: any[]) {
  const msg = parts.map(p => typeof p === 'string' ? p : JSON.stringify(p)).join(' ');
  const line = `[qqbot ${level}] ${msg}\n`;
  try { process.stderr.write(line); } catch {}
}

// ============================================================================
// HTTP helpers
// ============================================================================

function jsonRequest(url: string, method: string, body?: any, headers?: Record<string, string>): Promise<any> {
  return new Promise((resolve, reject) => {
    const u = new URL(url);
    const payload = body ? JSON.stringify(body) : undefined;
    const opts: https.RequestOptions = {
      hostname: u.hostname,
      path: u.pathname + u.search,
      method,
      headers: {
        'Content-Type': 'application/json',
        ...(payload ? { 'Content-Length': Buffer.byteLength(payload).toString() } : {}),
        ...headers,
      },
    };
    const req = https.request(opts, res => {
      let data = '';
      res.on('data', chunk => data += chunk);
      res.on('end', () => {
        try {
          const json = JSON.parse(data);
          if (res.statusCode && res.statusCode >= 400) {
            reject(new Error(`QQ API ${res.statusCode}: ${data.slice(0, 200)}`));
          } else {
            resolve(json);
          }
        } catch {
          reject(new Error(`QQ API parse error: ${data.slice(0, 200)}`));
        }
      });
    });
    req.on('error', reject);
    req.setTimeout(15000, () => { req.destroy(); reject(new Error('request timeout')); });
    if (payload) req.write(payload);
    req.end();
  });
}

// ============================================================================
// QQBotClient
// ============================================================================

export class QQBotClient extends EventEmitter {
  private readonly config: QQBotConfig;
  private readonly apiBase: string;

  // OAuth2 token
  private token = '';
  private tokenExpiry = 0;

  // WebSocket
  private ws: WebSocket | null = null;
  private sessionId = '';
  private lastSeq = 0;
  private heartbeatInterval: ReturnType<typeof setInterval> | null = null;
  private heartbeatMs = DEFAULT_HEARTBEAT_MS;
  private heartbeatAcked = true;

  // State
  private running = false;
  private reconnecting = false;

  // msg_seq tracking
  private msgSeqMap = new Map<string, { seq: number; createdAt: number }>();

  constructor(config: QQBotConfig) {
    super();
    this.config = config;
    this.apiBase = config.sandbox ? API_BASE_SANDBOX : API_BASE_PROD;
    this.setMaxListeners(20);
  }

  // --------------------------------------------------------------------------
  // Token management
  // --------------------------------------------------------------------------

  private async refreshToken(): Promise<void> {
    const resp = await jsonRequest(TOKEN_URL, 'POST', {
      appId: this.config.appId,
      clientSecret: this.config.appSecret,
    });
    this.token = resp.access_token;
    const expiresIn = parseInt(resp.expires_in, 10) || 7200;
    this.tokenExpiry = Date.now() + expiresIn * 1000;
    log('info', `token refreshed, expires in ${expiresIn}s`);
  }

  private async getToken(): Promise<string> {
    if (!this.token || Date.now() > this.tokenExpiry - TOKEN_REFRESH_MARGIN_MS) {
      await this.refreshToken();
    }
    return this.token;
  }

  private authHeaders(): Record<string, string> {
    return { Authorization: `QQBot ${this.token}` };
  }

  // --------------------------------------------------------------------------
  // Public API
  // --------------------------------------------------------------------------

  async start(): Promise<void> {
    this.running = true;
    await this.refreshToken();
    await this.connectGateway();
  }

  async stop(): Promise<void> {
    this.running = false;
    this.stopHeartbeat();
    if (this.ws) {
      try { this.ws.close(1000); } catch {}
      this.ws = null;
    }
  }

  async sendText(ctx: QQReplyContext, text: string): Promise<void> {
    const token = await this.getToken();
    const chunks = chunkText(text, MAX_MSG_LENGTH);

    for (const chunk of chunks) {
      const seq = this.nextMsgSeq(ctx.eventMsgId);
      const body: any = {
        content: chunk,
        msg_type: 0,
        msg_id: ctx.eventMsgId,
        msg_seq: seq,
      };

      const endpoint = ctx.messageType === 'group'
        ? `${this.apiBase}/v2/groups/${ctx.groupOpenId}/messages`
        : `${this.apiBase}/v2/users/${ctx.userOpenId}/messages`;

      try {
        await jsonRequest(endpoint, 'POST', body, { Authorization: `QQBot ${token}` });
      } catch (err: any) {
        // Retry once on 401 with refreshed token
        if (err.message?.includes('401')) {
          await this.refreshToken();
          await jsonRequest(endpoint, 'POST', body, this.authHeaders());
        } else {
          throw err;
        }
      }
    }
  }

  // --------------------------------------------------------------------------
  // Gateway
  // --------------------------------------------------------------------------

  private async connectGateway(): Promise<void> {
    const token = await this.getToken();
    const gwResp = await jsonRequest(
      `${this.apiBase}/gateway/bot`,
      'GET',
      undefined,
      { Authorization: `QQBot ${token}` },
    );
    const gwUrl = gwResp.url;
    if (!gwUrl) throw new Error('No gateway URL in response');

    log('info', `connecting to gateway: ${gwUrl}`);

    return new Promise((resolve, reject) => {
      const ws = new WebSocket(gwUrl);
      this.ws = ws;

      let identified = false;

      ws.on('open', () => log('info', 'ws connected'));

      ws.on('message', (data) => {
        try {
          const payload = JSON.parse(data.toString());
          this.handlePayload(payload, () => {
            if (!identified) {
              identified = true;
              resolve();
            }
          });
        } catch (err: any) {
          log('error', 'ws parse error:', err.message);
        }
      });

      ws.on('close', (code, reason) => {
        log('info', `ws closed code=${code} reason=${reason?.toString()?.slice(0, 80)}`);
        this.stopHeartbeat();
        if (this.running && !this.reconnecting) {
          this.scheduleReconnect();
        }
        if (!identified) reject(new Error(`ws closed before ready: ${code}`));
      });

      ws.on('error', (err) => {
        log('error', 'ws error:', err.message);
        if (!identified) reject(err);
      });
    });
  }

  private handlePayload(payload: any, onReady: () => void): void {
    const op = payload.op;
    if (payload.s) this.lastSeq = payload.s;

    switch (op) {
      case OP_HELLO: {
        this.heartbeatMs = payload.d?.heartbeat_interval || DEFAULT_HEARTBEAT_MS;
        log('info', `hello, heartbeat=${this.heartbeatMs}ms`);
        this.sendIdentify();
        break;
      }
      case OP_DISPATCH: {
        const t = payload.t;
        if (t === 'READY') {
          this.sessionId = payload.d?.session_id || '';
          log('info', `ready, session=${this.sessionId.slice(0, 16)}`);
          this.startHeartbeat();
          this.emit('ready');
          onReady();
        } else if (t === 'RESUMED') {
          log('info', 'resumed');
          this.startHeartbeat();
          this.emit('ready');
          onReady();
        } else if (t === 'GROUP_AT_MESSAGE_CREATE') {
          this.handleGroupMessage(payload.d);
        } else if (t === 'C2C_MESSAGE_CREATE') {
          this.handleC2CMessage(payload.d);
        }
        break;
      }
      case OP_HEARTBEAT_ACK:
        this.heartbeatAcked = true;
        break;
      case OP_RECONNECT:
        log('info', 'server requested reconnect');
        this.ws?.close();
        break;
      case OP_INVALID_SESSION: {
        log('warn', 'invalid session, re-identifying');
        this.sessionId = '';
        this.sendIdentify();
        break;
      }
    }
  }

  private sendIdentify(): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;

    if (this.sessionId) {
      // Resume
      log('info', 'resuming session');
      this.ws.send(JSON.stringify({
        op: OP_RESUME,
        d: {
          token: `QQBot ${this.token}`,
          session_id: this.sessionId,
          seq: this.lastSeq,
        },
      }));
    } else {
      // Fresh identify
      this.ws.send(JSON.stringify({
        op: OP_IDENTIFY,
        d: {
          token: `QQBot ${this.token}`,
          intents: INTENT_GROUP_AND_C2C,
          shard: [0, 1],
        },
      }));
    }
  }

  // --------------------------------------------------------------------------
  // Heartbeat
  // --------------------------------------------------------------------------

  private startHeartbeat(): void {
    this.stopHeartbeat();
    this.heartbeatAcked = true;
    this.heartbeatInterval = setInterval(() => {
      if (!this.heartbeatAcked) {
        log('warn', 'heartbeat ACK missed, reconnecting');
        this.ws?.close();
        return;
      }
      this.heartbeatAcked = false;
      if (this.ws?.readyState === WebSocket.OPEN) {
        this.ws.send(JSON.stringify({ op: OP_HEARTBEAT, d: this.lastSeq || null }));
      }
    }, this.heartbeatMs);
  }

  private stopHeartbeat(): void {
    if (this.heartbeatInterval) {
      clearInterval(this.heartbeatInterval);
      this.heartbeatInterval = null;
    }
  }

  // --------------------------------------------------------------------------
  // Reconnection
  // --------------------------------------------------------------------------

  private async scheduleReconnect(): Promise<void> {
    if (this.reconnecting || !this.running) return;
    this.reconnecting = true;

    for (let attempt = 1; attempt <= MAX_RECONNECT_ATTEMPTS; attempt++) {
      if (!this.running) break;
      const delay = Math.min(attempt * 2000, 60000);
      log('info', `reconnect attempt ${attempt}/${MAX_RECONNECT_ATTEMPTS} in ${delay}ms`);
      await sleep(delay);
      if (!this.running) break;

      try {
        await this.refreshToken();
        await this.connectGateway();
        log('info', 'reconnected');
        this.reconnecting = false;
        this.emit('status', 'running');
        return;
      } catch (err: any) {
        log('error', `reconnect failed: ${err.message}`);
        this.emit('status', 'reconnecting');
      }
    }

    this.reconnecting = false;
    log('error', 'max reconnect attempts reached');
    this.emit('error', new Error('max reconnect attempts'));
  }

  // --------------------------------------------------------------------------
  // Event handlers
  // --------------------------------------------------------------------------

  private handleGroupMessage(d: any): void {
    if (!d) return;
    const msgId = d.id;
    const groupOpenId = d.group_openid;
    const memberOpenId = d.author?.member_openid;
    let content: string = d.content || '';

    if (!msgId || !memberOpenId) return;

    // Strip @mention prefix: content starts with something like " message"
    // The official API already delivers only @bot messages, but content may have leading space
    content = content.replace(/^\s+/, '');

    const ctx: QQReplyContext = {
      messageType: 'group',
      groupOpenId,
      userOpenId: memberOpenId,
      eventMsgId: msgId,
    };

    const msg: QQMessage = {
      id: msgId,
      text: content,
      replyCtx: ctx,
      senderName: memberOpenId.slice(0, 8),
      imagePaths: [],
    };

    this.emit('message', msg);
  }

  private handleC2CMessage(d: any): void {
    if (!d) return;
    const msgId = d.id;
    const userOpenId = d.author?.user_openid;
    const content: string = d.content || '';

    if (!msgId || !userOpenId) return;

    const ctx: QQReplyContext = {
      messageType: 'c2c',
      userOpenId,
      eventMsgId: msgId,
    };

    const msg: QQMessage = {
      id: msgId,
      text: content.trim(),
      replyCtx: ctx,
      senderName: userOpenId.slice(0, 8),
      imagePaths: [],
    };

    this.emit('message', msg);
  }

  // --------------------------------------------------------------------------
  // msg_seq management
  // --------------------------------------------------------------------------

  private nextMsgSeq(eventMsgId: string): number {
    const now = Date.now();
    // Evict stale entries
    if (this.msgSeqMap.size > 200) {
      for (const [k, v] of this.msgSeqMap) {
        if (now - v.createdAt > MSG_SEQ_TTL_MS) this.msgSeqMap.delete(k);
      }
    }

    let entry = this.msgSeqMap.get(eventMsgId);
    if (!entry) {
      entry = { seq: 0, createdAt: now };
      this.msgSeqMap.set(eventMsgId, entry);
    }
    entry.seq++;
    return entry.seq;
  }
}

// ============================================================================
// Helpers
// ============================================================================

function sleep(ms: number): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, ms));
}

function chunkText(text: string, maxLen: number): string[] {
  if (text.length <= maxLen) return [text];
  const chunks: string[] = [];
  let start = 0;
  while (start < text.length) {
    chunks.push(text.slice(start, start + maxLen));
    start += maxLen;
  }
  return chunks;
}
