import {
  cpSync,
  createWriteStream,
  existsSync,
  mkdirSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import http from 'node:http';
import https from 'node:https';
import { dirname, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

const target = process.argv[2] || '';
const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const binDir = join(root, 'src-tauri', 'binaries');
const destExe = join(binDir, 'claude-mem.exe');
const destPlugin = join(binDir, 'plugin');
const claudeMemRepo = process.env.CLAUDE_MEM_REPO || 'nevermorewish/claude-mem';
const claudeMemReleaseTag = process.env.CLAUDE_MEM_RELEASE_TAG || '';
const claudeMemReleaseAsset = process.env.CLAUDE_MEM_RELEASE_ASSET || 'claude-mem-windows-x64.zip';
const claudeMemUrl = process.env.CLAUDE_MEM_URL || (
  claudeMemReleaseTag
    ? `https://github.com/${claudeMemRepo}/releases/download/${claudeMemReleaseTag}/${claudeMemReleaseAsset}`
    : ''
);

function isWindowsTarget() {
  return target.includes('windows') || (target === '' && process.platform === 'win32');
}

function normalize(value) {
  return value && value.trim() ? resolve(value.trim()) : null;
}

function existingDir(path) {
  return path && existsSync(path) && statSync(path).isDirectory() ? path : null;
}

function existingFile(path) {
  return path && existsSync(path) && statSync(path).isFile() ? path : null;
}

function hasPlugin(path) {
  return Boolean(path && existingFile(join(path, 'scripts', 'worker-service.cjs')));
}

function hasPreparedResources() {
  return Boolean(existingFile(destExe) && existingDir(destPlugin) && hasPlugin(destPlugin));
}

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

function sourceHomes() {
  const homes = [
    normalize(process.env.CLAUDE_MEM_HOME),
    normalize(process.env.FROGCLAW_CLAUDE_MEM_HOME),
  ];

  const explicitExe = normalize(process.env.CLAUDE_MEM_EXE || process.env.FROGCLAW_CLAUDE_MEM_EXE);
  if (explicitExe) homes.push(dirname(explicitExe));

  if (process.platform === 'win32') {
    homes.push('E:\\frogclaw\\claude-mem');
  }
  homes.push(resolve(root, '..', 'claude-mem'));
  homes.push(resolve(root, 'claude-mem'));

  return [...new Set(homes.filter(Boolean))];
}

function resolveSourceInDir(home) {
  const plugin = existingDir(join(home, 'plugin'));
  if (!hasPlugin(plugin)) return null;

  const candidates = [
    join(home, 'claude-mem.exe'),
    join(home, 'dist', 'binaries', 'claude-mem.exe'),
  ];
  const binariesDir = join(home, 'dist', 'binaries');
  if (existingDir(binariesDir)) {
    const names = ['worker-service'].flatMap((prefix) => {
      try {
        return Array.from(
          new Set(
            readdirSync(binariesDir)
            .filter((name) => name.startsWith(prefix) && name.endsWith('.exe'))
            .sort()
            .reverse()
          )
        ).map((name) => join(binariesDir, name));
      } catch {
        return [];
      }
    });
    candidates.push(...names);
  }

  const exe = candidates.map(existingFile).find(Boolean);
  return exe ? { exe, plugin } : null;
}

function resolveSource() {
  const explicitExe = existingFile(normalize(process.env.CLAUDE_MEM_EXE || process.env.FROGCLAW_CLAUDE_MEM_EXE));
  const explicitPlugin = existingDir(normalize(process.env.CLAUDE_MEM_PLUGIN_DIR));
  if (explicitExe) {
    const plugin = explicitPlugin ?? existingDir(join(dirname(explicitExe), 'plugin'));
    return plugin && hasPlugin(plugin)
      ? { exe: explicitExe, plugin }
      : { exe: explicitExe, plugin };
  }

  for (const home of sourceHomes()) {
    const source = resolveSourceInDir(home);
    if (source) return source;
  }

  return null;
}

function findPackageSource(extractedDir) {
  const direct = resolveSourceInDir(extractedDir);
  if (direct) return direct;

  const candidates = [
    join(extractedDir, 'claude-mem'),
    join(extractedDir, 'package'),
  ];
  for (const candidate of candidates) {
    const source = resolveSourceInDir(candidate);
    if (source) return source;
  }
  return null;
}

function powershell() {
  for (const candidate of ['pwsh', 'powershell']) {
    const result = spawnSync(candidate, ['-NoProfile', '-Command', '$PSVersionTable.PSVersion.ToString()'], {
      stdio: 'ignore',
      shell: process.platform === 'win32',
    });
    if (result.status === 0) return candidate;
  }
  return null;
}

function extractZip(zipPath, outputDir) {
  mkdirSync(outputDir, { recursive: true });
  if (process.platform === 'win32') {
    const ps = powershell();
    if (!ps) throw new Error('PowerShell is required to extract claude-mem release packages on Windows.');
    run(ps, [
      '-NoProfile',
      '-Command',
      `Expand-Archive -LiteralPath '${zipPath.replaceAll("'", "''")}' -DestinationPath '${outputDir.replaceAll("'", "''")}' -Force`,
    ]);
    return;
  }

  const tar = spawnSync('tar', ['-xf', zipPath, '-C', outputDir], { stdio: 'inherit' });
  if (tar.status === 0) return;
  run('unzip', ['-q', zipPath, '-d', outputDir]);
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
    const request = client.get(url, { headers }, (response) => {
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

function bundleSource(source) {
  if (!source.plugin || !hasPlugin(source.plugin)) {
    throw new Error(`claude-mem plugin directory not found or incomplete next to ${source.exe}`);
  }

  cpSync(source.exe, destExe, { force: true });
  rmSync(destPlugin, { recursive: true, force: true });
  cpSync(source.plugin, destPlugin, { recursive: true, force: true });
  console.log(`Bundled claude-mem executable: ${source.exe} -> ${destExe}`);
  console.log(`Bundled claude-mem plugin: ${source.plugin} -> ${destPlugin}`);
}

async function bundleReleasePackage() {
  if (!claudeMemUrl) return false;

  const workDir = join(tmpdir(), `frogclaw-claude-mem-${process.pid}`);
  const zipPath = join(workDir, claudeMemReleaseAsset);
  const extractDir = join(workDir, 'extract');
  rmSync(workDir, { recursive: true, force: true });
  mkdirSync(workDir, { recursive: true });

  try {
    console.log(`Downloading claude-mem package: ${claudeMemUrl}`);
    await download(claudeMemUrl, zipPath);
    if (statSync(zipPath).size <= 0) {
      throw new Error(`Downloaded claude-mem package is empty: ${zipPath}`);
    }
    extractZip(zipPath, extractDir);
    const source = findPackageSource(extractDir);
    if (!source) {
      throw new Error(`claude-mem.exe and plugin/ were not found in ${zipPath}`);
    }
    bundleSource(source);
    return true;
  } finally {
    rmSync(workDir, { recursive: true, force: true });
  }
}

mkdirSync(binDir, { recursive: true });

if (!isWindowsTarget()) {
  if (!existsSync(destExe)) writeFileSync(destExe, '');
  mkdirSync(destPlugin, { recursive: true });
  if (!hasPlugin(destPlugin)) writeFileSync(join(destPlugin, '.placeholder'), '');
  console.log(`Prepared non-Windows claude-mem placeholders in ${binDir}`);
  process.exit(0);
}

if (!target && hasPreparedResources()) {
  console.log(`Using already prepared claude-mem resources in ${binDir}`);
  process.exit(0);
}

if (target && await bundleReleasePackage()) {
  process.exit(0);
}

const source = resolveSource();
if (source) {
  bundleSource(source);
  process.exit(0);
}

if (await bundleReleasePackage()) {
  process.exit(0);
}

if (hasPreparedResources()) {
  console.log(`Using already prepared claude-mem resources in ${binDir}`);
  process.exit(0);
}

throw new Error(
  'claude-mem resources not found. Set CLAUDE_MEM_RELEASE_TAG, CLAUDE_MEM_URL, CLAUDE_MEM_HOME, FROGCLAW_CLAUDE_MEM_HOME, CLAUDE_MEM_EXE, or FROGCLAW_CLAUDE_MEM_EXE.'
);
