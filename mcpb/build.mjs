// Node 20+, no dependencies. All paths are local; this script makes no network requests.
import { readFile, writeFile, mkdir, mkdtemp, rm, stat, realpath } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { writeZip, readZip, sha256 } from './zip.mjs';
import { inside, mcpFolders, startMcp } from './mcp-client.mjs';

export const root = fileURLToPath(new URL('../', import.meta.url));
export const VERSION = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?![\s\S])/;
export const BUNDLE = 'cutbolt-windows-x64.mcpb';
export const json = value => Buffer.from(JSON.stringify(value, null, 2) + '\n');
export const readJSON = async file => JSON.parse(await readFile(file, 'utf8'));
export const readText = async file => Buffer.from((await readFile(file, 'utf8')).replace(/\r\n?/g, '\n'));

export async function cargoVersion(base = root) {
  const toml = await readFile(path.join(base, 'Cargo.toml'), 'utf8');
  const section = /^\[package\][ \t]*\r?\n([\s\S]*?)(?=^\[|$(?![\s\S]))/m.exec(toml)?.[1];
  const version = /^[ \t]*version[ \t]*=[ \t]*["']([^"']+)["'][ \t]*(?:#.*)?\r?$/m.exec(section ?? '')?.[1];
  if (!version || !VERSION.test(version)) throw new Error("Cargo.toml [package] version must have the form X.Y.Z.");
  return version;
}

export async function writeUnchangedOrNew(file, bytes) {
  const existing = await readFile(file).catch(error => { if (error.code !== 'ENOENT') throw error; return null; });
  if (existing) {
    if (!existing.equals(bytes)) throw new Error(`${file} already exists with different contents; choose a fresh --out.`);
    return;
  }
  await mkdir(path.dirname(file), { recursive: true });
  await writeFile(file, bytes, { flag: 'wx' });
}

export async function readNotices(notices) {
  if (!notices) throw new Error('--notices <file> is required; generate it with mcpb/notices.mjs for the build target.');
  const bytes = await readText(notices);
  if (!bytes.toString('utf8').trim()) throw new Error(`Empty notices file: ${notices}`);
  return bytes;
}

function shortDescription(description) {
  if (typeof description !== 'string' || !description.trim()) throw new Error('New tools need a server description.');
  const line = description.trim().replace(/\s+/g, ' ');
  const sentence = /^.*?[.!?](?=\s|$)/u.exec(line)?.[0] ?? line;
  if (sentence.length <= 160) return sentence;
  const end = sentence.lastIndexOf(' ', 160);
  if (end < 1) throw new Error('Tool description has no word boundary within 160 characters.');
  return sentence.slice(0, end);
}

export function mergeTools(overrides, liveTools) {
  if (!Array.isArray(liveTools) || liveTools.length === 0) throw new Error('MCP server lists no tools.');
  const curated = new Map(overrides.map(tool => [tool.name, tool.description]));
  const names = new Set(), added = [];
  const tools = liveTools.map(tool => {
    const name = tool?.name;
    if (typeof name !== 'string' || name !== name.trim() || !/^cutbolt_\w+$/.test(name)) throw new Error(`Invalid tool name: ${JSON.stringify(name)}`);
    if (names.has(name)) throw new Error(`Duplicate tool name: ${name}`);
    names.add(name);
    if (!curated.has(name)) added.push(name);
    return { name, description: curated.has(name) ? curated.get(name) : shortDescription(tool.description) };
  });
  return { tools, added, dropped: overrides.filter(tool => !names.has(tool.name)).map(tool => tool.name) };
}

export async function discoverTools({ manifest, binary, base = root }) {
  const tempRoot = path.resolve(base, 'target');
  await mkdir(tempRoot, { recursive: true });
  const temp = await mkdtemp(path.join(tempRoot, 'mcpb-build-'));
  let client;
  try {
    client = startMcp({ manifest, binary, ...await mcpFolders(temp) });
    await client.initialize();
    return await client.listTools();
  } finally {
    try { await client?.stop(); } finally {
      if (!inside(tempRoot, temp)) throw new Error('Refusing to remove a folder outside target/');
      await rm(temp, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    }
  }
}

export async function assembleMcpb({ base = root, binary, notices } = {}, discover = discoverTools) {
  const template = await readJSON(path.join(base, 'mcpb/manifest.json'));
  if (Object.hasOwn(template, 'version')) throw new Error('mcpb/manifest.json must not contain version; Cargo.toml supplies it.');
  const version = await cargoVersion(base);
  if (!binary || !(await stat(binary).catch(error => { if (error.code !== 'ENOENT') throw error; return null; }))?.isFile()) {
    throw new Error(`Missing binary: ${binary ?? '(provide --binary target/release/cutbolt.exe)'}`);
  }
  const noticeBytes = await readNotices(notices);
  const manifest = { ...template, version };
  const catalog = mergeTools(template.tools, await discover({ manifest, binary, base }));
  manifest.tools = catalog.tools;
  console.log(`Core tools: added ${catalog.added.join(', ') || '(none)'}; dropped ${catalog.dropped.join(', ') || '(none)'}`);
  const files = new Map([
    ['manifest.json', json(manifest)], ['server/cutbolt.exe', await readFile(binary)],
    ['LICENSE', await readText(path.join(base, 'LICENSE'))],
    ['DEPENDENCIES.md', await readText(path.join(base, 'docs/DEPENDENCIES.md'))],
    ['THIRD_PARTY_NOTICES.md', noticeBytes]
  ]);
  const bytes = writeZip(files);
  return { bytes, manifest, files: [...files.keys()].sort(), hash: sha256(bytes), added: catalog.added, dropped: catalog.dropped };
}

export async function buildMcpb({ base = root, binary, notices, out = path.join(base, 'target/dist/mcpb') } = {}, discover = discoverTools) {
  const build = await assembleMcpb({ base, binary, notices }, discover);
  const bundle = path.resolve(out, BUNDLE);
  await writeUnchangedOrNew(bundle, build.bytes);
  await writeUnchangedOrNew(`${bundle}.sha256`, Buffer.from(`${build.hash}  ${BUNDLE}\n`));
  return { ...build, bundle };
}

const samePath = (a, b) => process.platform === 'win32' ? a.toLowerCase() === b.toLowerCase() : a === b;
// Resolve existing parents too, so a junction alias cannot disguise an output collision.
async function canonical(file) {
  try { return await realpath(file); } catch (error) {
    if (error.code !== 'ENOENT') throw error;
    const parent = path.dirname(file);
    if (parent === file) throw error;
    return path.join(await canonical(parent), path.basename(file));
  }
}

export async function registryEntry({ base = root, tag, bundle, out = path.join(base, 'target/dist/mcpb/server.registry.json') } = {}) {
  if (typeof tag !== 'string' || !tag.startsWith('v') || !VERSION.test(tag.slice(1))) throw new Error('Tag must have the form vX.Y.Z.');
  if (!bundle) throw new Error('--bundle is required.');
  const metadataPath = path.resolve(base, 'server.json'), bundlePath = path.resolve(bundle), destination = path.resolve(out);
  for (const source of [metadataPath, bundlePath]) {
    const outputStat = await stat(destination).catch(e => { if (e.code !== 'ENOENT') throw e; return null; });
    const sourceStat = await stat(source);
    if (samePath(await canonical(destination), await canonical(source)) ||
        (outputStat && outputStat.dev === sourceStat.dev && outputStat.ino === sourceStat.ino)) {
      throw new Error('--out must not overwrite server.json or the bundle.');
    }
  }
  const server = await readJSON(metadataPath), version = await cargoVersion(base);
  if (Object.hasOwn(server, 'packages')) throw new Error('server.json must remain metadata-only (no packages property).');
  if (`v${version}` !== tag) throw new Error('Tag and Cargo.toml versions must match.');
  const bytes = await readFile(bundlePath), manifestFile = readZip(bytes).get('manifest.json');
  if (!manifestFile) throw new Error('Bundle has no manifest.json.');
  if (JSON.parse(manifestFile.read()).version !== version) throw new Error('Bundle manifest version must match the tag and Cargo.toml.');
  // Deliberately ignore the source metadata's version: Cargo.toml is the release authority.
  const entry = { ...server, version, packages: [{
    registryType: 'mcpb', identifier: `https://github.com/skulitom/Cutbolt/releases/download/${tag}/${BUNDLE}`,
    version, fileSha256: sha256(bytes), transport: { type: 'stdio' }
  }] };
  await writeUnchangedOrNew(destination, json(entry));
  return entry;
}

async function main(argv) {
  const options = {};
  for (let i = 0; i < argv.length; i++) {
    const key = argv[i].slice(2);
    if (Object.hasOwn(options, key)) throw new Error(`Duplicate option: ${argv[i]}`);
    if (argv[i] === '--registry-entry') options[key] = true;
    else if (['--binary', '--notices', '--out', '--tag', '--bundle'].includes(argv[i]) && argv[i + 1] && !argv[i + 1].startsWith('--')) options[key] = argv[++i];
    else throw new Error(`Unknown or incomplete option: ${argv[i]}`);
  }
  if (options['registry-entry']) {
    if (options.binary || options.notices) throw new Error('--binary and --notices are only for bundle builds.');
    const entry = await registryEntry(options);
    console.log(`Registry entry: ${entry.name} ${entry.version}, SHA-256 ${entry.packages[0].fileSha256}`);
  } else {
    if (options.tag || options.bundle) throw new Error('--tag and --bundle require --registry-entry.');
    const build = await buildMcpb(options);
    console.log(`${build.bundle}\n${build.bytes.length} bytes; SHA-256 ${build.hash}\n${build.files.join('\n')}`);
  }
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch(error => { console.error(error.message); process.exitCode = 1; });
}
