import { createWriteStream, existsSync, mkdirSync, copyFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import http from 'node:http';
import https from 'node:https';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

const target = process.argv[2] || '';
const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const binDir = join(root, 'src-tauri', 'binaries');
const dest = join(binDir, 'codex-app-server.exe');
const codexRepo = process.env.CODEX_REPO || 'https://github.com/nevermorewish/codex.git';
const codexRef = process.env.CODEX_REF || '';
const appServerRepo = process.env.CODEX_APP_SERVER_REPO || 'nevermorewish/codex';
const appServerReleaseTag = process.env.CODEX_APP_SERVER_RELEASE_TAG || '';
const appServerUrl = process.env.CODEX_APP_SERVER_URL || (
  appServerReleaseTag
    ? `https://github.com/${appServerRepo}/releases/download/${appServerReleaseTag}/codex-app-server-${target}.exe`
    : ''
);
const allowSourceBuild = process.env.CODEX_APP_SERVER_ALLOW_SOURCE_BUILD === '1';

function run(cmd, args, options = {}) {
  const result = spawnSync(cmd, args, {
    stdio: 'inherit',
    shell: process.platform === 'win32',
    ...options,
  });
  if (result.status !== 0) {
    throw new Error(`${cmd} ${args.join(' ')} failed with status ${result.status}`);
  }
}

function download(url, output, redirectCount = 0) {
  if (redirectCount > 5) {
    throw new Error(`Too many redirects while downloading ${url}`);
  }

  return new Promise((resolveDownload, rejectDownload) => {
    const client = url.startsWith('https:') ? https : http;
    const requestUrl = new URL(url);
    const headers = {
      'user-agent': 'frogclawclient-release-build',
      ...(process.env.GITHUB_TOKEN && requestUrl.hostname === 'github.com'
        ? { authorization: `Bearer ${process.env.GITHUB_TOKEN}` }
        : {}),
    };
    const request = client.get(url, {
      headers,
    }, (response) => {
      const statusCode = response.statusCode || 0;
      const location = response.headers.location;
      if ([301, 302, 303, 307, 308].includes(statusCode) && location) {
        response.resume();
        const redirected = new URL(location, url).toString();
        download(redirected, output, redirectCount + 1).then(resolveDownload, rejectDownload);
        return;
      }
      if (statusCode < 200 || statusCode >= 300) {
        response.resume();
        rejectDownload(new Error(`Download failed with HTTP ${statusCode}: ${url}`));
        return;
      }

      const file = createWriteStream(output);
      response.pipe(file);
      file.on('finish', () => {
        file.close(resolveDownload);
      });
      file.on('error', rejectDownload);
    });
    request.on('error', rejectDownload);
  });
}

mkdirSync(binDir, { recursive: true });

if (!target.includes('windows')) {
  if (!existsSync(dest)) {
    writeFileSync(dest, '');
    console.log(`Created non-Windows placeholder: ${dest}`);
  } else {
    console.log(`Using existing Codex app-server resource placeholder: ${dest}`);
  }
  process.exit(0);
}

if (appServerUrl) {
  try {
    rmSync(dest, { force: true });
    console.log(`Downloading Codex app-server: ${appServerUrl}`);
    await download(appServerUrl, dest);
    if (statSync(dest).size <= 0) {
      throw new Error(`Downloaded Codex app-server is empty: ${dest}`);
    }
    console.log(`Bundled ${appServerUrl} -> ${dest}`);
    process.exit(0);
  } catch (error) {
    rmSync(dest, { force: true });
    if (!allowSourceBuild) {
      throw error;
    }
    console.warn(`Prebuilt Codex app-server download failed; falling back to source build: ${error.message}`);
  }
}

if (!allowSourceBuild) {
  throw new Error('Set CODEX_APP_SERVER_RELEASE_TAG or CODEX_APP_SERVER_URL to download a prebuilt codex-app-server.exe.');
}

const workDir = join(tmpdir(), `frogclaw-codex-${target}`);
rmSync(workDir, { recursive: true, force: true });

run('git', ['init', workDir]);
run('git', ['-C', workDir, 'remote', 'add', 'origin', codexRepo]);
if (codexRef) {
  run('git', ['-C', workDir, 'fetch', '--depth', '1', 'origin', codexRef]);
  run('git', ['-C', workDir, 'checkout', '--detach', 'FETCH_HEAD']);
} else {
  run('git', ['-C', workDir, 'fetch', '--depth', '1', 'origin', 'HEAD']);
  run('git', ['-C', workDir, 'checkout', '--detach', 'FETCH_HEAD']);
}

const manifest = join(workDir, 'codex-rs', 'Cargo.toml');
run('cargo', [
  'build',
  '--manifest-path',
  manifest,
  '-p',
  'codex-app-server',
  '--release',
  '--target',
  target,
]);

const built = join(workDir, 'codex-rs', 'target', target, 'release', 'codex-app-server.exe');
if (!existsSync(built)) {
  throw new Error(`codex-app-server.exe was not produced at ${built}`);
}

copyFileSync(built, dest);
console.log(`Bundled ${built} -> ${dest}`);
