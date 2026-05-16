import { cpSync, existsSync, mkdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const target = process.argv[2] || '';
const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const binDir = join(root, 'src-tauri', 'binaries');
const destExe = join(binDir, 'claude-mem.exe');
const destPlugin = join(binDir, 'plugin');

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

function resolveSource() {
  const explicitExe = existingFile(normalize(process.env.CLAUDE_MEM_EXE || process.env.FROGCLAW_CLAUDE_MEM_EXE));
  const explicitPlugin = existingDir(normalize(process.env.CLAUDE_MEM_PLUGIN_DIR));
  if (explicitExe) {
    return {
      exe: explicitExe,
      plugin: explicitPlugin ?? existingDir(join(dirname(explicitExe), 'plugin')),
    };
  }

  for (const home of sourceHomes()) {
    const exe = existingFile(join(home, 'claude-mem.exe'));
    const plugin = existingDir(join(home, 'plugin'));
    if (exe && plugin) return { exe, plugin };
  }

  return null;
}

mkdirSync(binDir, { recursive: true });

const source = resolveSource();
if (!source) {
  if (!isWindowsTarget()) {
    if (!existsSync(destExe)) writeFileSync(destExe, '');
    mkdirSync(destPlugin, { recursive: true });
    writeFileSync(join(destPlugin, '.placeholder'), '');
    console.log(`Created non-Windows claude-mem placeholders in ${binDir}`);
    process.exit(0);
  }
  throw new Error(
    'claude-mem resources not found. Set CLAUDE_MEM_HOME, FROGCLAW_CLAUDE_MEM_HOME, CLAUDE_MEM_EXE, or FROGCLAW_CLAUDE_MEM_EXE.'
  );
}

cpSync(source.exe, destExe, { force: true });
if (source.plugin) {
  rmSync(destPlugin, { recursive: true, force: true });
  cpSync(source.plugin, destPlugin, { recursive: true, force: true });
} else {
  throw new Error(`claude-mem plugin directory not found next to ${source.exe}`);
}

console.log(`Bundled claude-mem executable: ${source.exe} -> ${destExe}`);
console.log(`Bundled claude-mem plugin: ${source.plugin} -> ${destPlugin}`);
