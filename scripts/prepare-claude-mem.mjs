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

const target = process.argv[2] || process.env.TAURI_BUILD_TARGET || '';
const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const binDir = join(root, 'src-tauri', 'binaries');
const binaryName = binaryNameForTarget(target);
const destExe = join(binDir, binaryName);
const staleExe = join(binDir, binaryName === 'claude-mem.exe' ? 'claude-mem' : 'claude-mem.exe');
const destPlugin = join(binDir, 'plugin');
const claudeMemRepo = process.env.CLAUDE_MEM_REPO || 'nevermorewish/claude-mem';
const claudeMemReleaseTag = process.env.CLAUDE_MEM_RELEASE_TAG || '';
const claudeMemReleaseAsset = process.env.CLAUDE_MEM_RELEASE_ASSET || releaseAssetForTarget(target);
const claudeMemUrl = process.env.CLAUDE_MEM_URL || (
  claudeMemReleaseTag
    ? `https://github.com/${claudeMemRepo}/releases/download/${claudeMemReleaseTag}/${claudeMemReleaseAsset}`
    : ''
);

function targetPlatform(value) {
  if (value.includes('windows')) return 'windows';
  if (value.includes('apple-darwin')) return 'macos';
  if (value.includes('linux')) return 'linux';
  if (process.platform === 'win32') return 'windows';
  if (process.platform === 'darwin') return 'macos';
  return 'linux';
}

function targetArch(value) {
  if (value.includes('aarch64') || value.includes('arm64')) return 'arm64';
  return 'x64';
}

function binaryNameForTarget(value) {
  return targetPlatform(value) === 'windows' ? 'claude-mem.exe' : 'claude-mem';
}

function releaseAssetForTarget(value) {
  return `claude-mem-${targetPlatform(value)}-${targetArch(value)}.zip`;
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

function existingNonEmptyFile(path) {
  return path && existsSync(path) && statSync(path).isFile() && statSync(path).size > 0 ? path : null;
}

function hasPlugin(path) {
  return Boolean(path && existingFile(join(path, 'scripts', 'worker-service.cjs')));
}

function hasRuntimeDeps(path) {
  return Boolean(path && existingFile(join(path, 'node_modules', 'zod', 'package.json')));
}

function hasPreparedCoreResources() {
  return Boolean(existingNonEmptyFile(destExe) && existingDir(destPlugin) && hasPlugin(destPlugin));
}

function hasPreparedResources() {
  return Boolean(hasPreparedCoreResources() && hasRuntimeDeps(destPlugin));
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
    join(home, binaryName),
    join(home, 'dist', 'binaries', binaryName),
  ];
  const binariesDir = join(home, 'dist', 'binaries');
  if (existingDir(binariesDir)) {
    const names = ['claude-mem', 'worker-service'].flatMap((prefix) => {
      try {
        return Array.from(
          new Set(
            readdirSync(binariesDir)
            .filter((name) => name.startsWith(prefix))
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

function delay(ms) {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, ms));
}

async function downloadWithRetry(url, output, attempts = 3) {
  let lastError = null;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      await download(url, output);
      return;
    } catch (error) {
      lastError = error;
      rmSync(output, { force: true });
      if (attempt >= attempts) break;
      const waitMs = 2000 * attempt;
      console.warn(`Download failed (${error.message}); retrying in ${waitMs}ms (${attempt}/${attempts})`);
      await delay(waitMs);
    }
  }
  throw lastError;
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
      response.on('error', rejectDownload);
    });
    request.on('error', rejectDownload);
    request.setTimeout(300000, () => {
      request.destroy(new Error(`Download timed out: ${url}`));
    });
  });
}

function bundleSource(source) {
  if (!source.plugin || !hasPlugin(source.plugin)) {
    throw new Error(`claude-mem plugin directory not found or incomplete next to ${source.exe}`);
  }

  cpSync(source.exe, destExe, { force: true });
  ensureStalePlaceholder();
  rmSync(destPlugin, { recursive: true, force: true });
  cpSync(source.plugin, destPlugin, { recursive: true, force: true });
  ensureRuntimeDeps(destPlugin, source.plugin);
  console.log(`Bundled claude-mem executable: ${source.exe} -> ${destExe}`);
  console.log(`Bundled claude-mem plugin: ${source.plugin} -> ${destPlugin}`);
}

function ensureStalePlaceholder() {
  rmSync(staleExe, { force: true });
  writeFileSync(staleExe, '');
}

function copyRuntimeDepFromSource(pluginDir, sourcePlugin, depName) {
  const source = join(sourcePlugin, 'node_modules', depName);
  if (!existingDir(source)) return false;
  const dest = join(pluginDir, 'node_modules', depName);
  rmSync(dest, { recursive: true, force: true });
  mkdirSync(dirname(dest), { recursive: true });
  cpSync(source, dest, { recursive: true, force: true });
  return true;
}

function ensureRuntimeDeps(pluginDir, sourcePlugin) {
  if (hasRuntimeDeps(pluginDir)) return;

  if (sourcePlugin && copyRuntimeDepFromSource(pluginDir, sourcePlugin, 'zod') && hasRuntimeDeps(pluginDir)) {
    console.log('Bundled claude-mem runtime dependency: zod');
    return;
  }

  const workDir = join(tmpdir(), `frogclaw-claude-mem-runtime-${process.pid}`);
  rmSync(workDir, { recursive: true, force: true });
  mkdirSync(workDir, { recursive: true });
  try {
    run('npm', [
      'install',
      '--prefix',
      workDir,
      '--no-save',
      '--omit=dev',
      '--ignore-scripts',
      '--package-lock=false',
      'zod@^4.3.6',
    ]);
    const source = join(workDir, 'node_modules', 'zod');
    if (!existingDir(source)) {
      throw new Error(`npm install completed but zod was not found in ${source}`);
    }
    const dest = join(pluginDir, 'node_modules', 'zod');
    rmSync(dest, { recursive: true, force: true });
    mkdirSync(dirname(dest), { recursive: true });
    cpSync(source, dest, { recursive: true, force: true });
    console.log(`Bundled claude-mem runtime dependency: ${source} -> ${dest}`);
  } finally {
    rmSync(workDir, { recursive: true, force: true });
  }
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
    await downloadWithRetry(claudeMemUrl, zipPath);
    if (statSync(zipPath).size <= 0) {
      throw new Error(`Downloaded claude-mem package is empty: ${zipPath}`);
    }
    extractZip(zipPath, extractDir);
    const source = findPackageSource(extractDir);
    if (!source) {
      throw new Error(`${binaryName} and plugin/ were not found in ${zipPath}`);
    }
    bundleSource(source);
    return true;
  } finally {
    rmSync(workDir, { recursive: true, force: true });
  }
}

mkdirSync(binDir, { recursive: true });

if (!target && hasPreparedResources()) {
  ensureStalePlaceholder();
  console.log(`Using already prepared claude-mem resources in ${binDir}`);
  process.exit(0);
}

if (target && await bundleReleasePackage()) {
  process.exit(0);
}

if (hasPreparedCoreResources()) {
  ensureStalePlaceholder();
  ensureRuntimeDeps(destPlugin, destPlugin);
  console.log(`Using already prepared claude-mem resources in ${binDir}`);
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

if (hasPreparedCoreResources()) {
  ensureStalePlaceholder();
  ensureRuntimeDeps(destPlugin, destPlugin);
  console.log(`Using already prepared claude-mem resources in ${binDir}`);
  process.exit(0);
}

throw new Error(
  'claude-mem resources not found. Set CLAUDE_MEM_RELEASE_TAG, CLAUDE_MEM_URL, CLAUDE_MEM_HOME, FROGCLAW_CLAUDE_MEM_HOME, CLAUDE_MEM_EXE, or FROGCLAW_CLAUDE_MEM_EXE.'
);
