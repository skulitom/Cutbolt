// Smoke-test the extracted binary with its manifest command and an existing, empty workspace.
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir, mkdtemp, readdir, lstat, rm } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { root, cargoVersion, VERSION } from './build.mjs';
import { readZip, sha256 } from './zip.mjs';
import { inside, mcpFolders, startMcp } from './mcp-client.mjs';

export function checkManifest(manifest, version) {
  for (const key of ['name', 'version', 'display_name', 'description', 'long_description', 'homepage', 'documentation', 'support']) {
    assert.ok(typeof manifest[key] === 'string' && manifest[key], `manifest.${key} is required`);
  }
  assert.equal(manifest.manifest_version, '0.3');
  assert.equal(manifest.name, 'cutbolt');
  assert.equal(manifest.display_name, 'Cutbolt');
  assert.match(manifest.version, VERSION);
  assert.equal(manifest.version, version, 'Manifest version must equal Cargo.toml version');
  assert.equal(manifest.author?.name, 'skulitom');
  assert.equal(manifest.author.url, 'https://github.com/skulitom');
  assert.equal(manifest.repository?.type, 'git');
  assert.equal(manifest.repository.url, 'https://github.com/skulitom/Cutbolt');
  assert.equal(manifest.license, 'MIT');
  assert.deepEqual(manifest.compatibility?.platforms, ['win32']);
  assert.equal(manifest.server?.type, 'binary');
  assert.equal(manifest.server.entry_point, 'server/cutbolt.exe');
  assert.deepEqual(manifest.server.mcp_config, {
    command: '${__dirname}/server/cutbolt.exe',
    args: ['--workspace', '${user_config.workspace}', 'mcp', '--tools', 'core']
  });
  const workspace = manifest.user_config?.workspace;
  assert.equal(workspace?.type, 'directory');
  assert.equal(workspace.required, true);
  assert.equal(workspace.default, '${HOME}');
  assert.ok(workspace.title && workspace.description);
  assert.ok(Array.isArray(manifest.tools) && manifest.tools.length > 0);
  assert.equal(new Set(manifest.tools.map(tool => tool.name)).size, manifest.tools.length, 'Duplicate manifest tools');
  assert.ok(manifest.tools.every(tool => /^cutbolt_\w+$/.test(tool.name) && typeof tool.description === 'string' && tool.description && !/[\r\n]/.test(tool.description)));
}

// Observe project files outside the temp root. Build caches and .git are watched at their
// top level, not walked recursively. This catches misplaced engine state without scanning
// gigabytes of build output; it is a smoke-test guard, not an OS filesystem sandbox.
async function footprint(base, excluded, shallow = new Set()) {
  const result = [];
  async function walk(dir) {
    for (const name of (await readdir(dir)).sort()) {
      const file = path.join(dir, name);
      if (file === excluded) continue;
      const info = await lstat(file), relative = path.relative(base, file);
      result.push([relative, info.isDirectory() ? 'dir' : [info.size, info.mtimeMs]]);
      if (info.isDirectory() && !info.isSymbolicLink() && !shallow.has(relative)) await walk(file);
    }
  }
  await walk(base);
  return result;
}

export async function verifyMcpb(bundle, { base = root, allowSkip = false } = {}) {
  if (process.platform !== 'win32') {
    if (allowSkip) return { skipped: true, message: 'Windows only: MCPB smoke test explicitly skipped.' };
    throw new Error('Windows only: this bundle contains a Windows x64 executable. Use --allow-skip to skip explicitly.');
  }
  const entries = readZip(await readFile(bundle));
  assert.deepEqual([...entries.keys()].sort(), ['DEPENDENCIES.md', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'manifest.json', 'server/cutbolt.exe']);
  const manifest = JSON.parse(entries.get('manifest.json').read());
  checkManifest(manifest, await cargoVersion(base));
  const tempRoot = path.resolve(base, 'target');
  await mkdir(tempRoot, { recursive: true });
  const temp = await mkdtemp(path.join(tempRoot, 'cutbolt-mcpb-'));
  let client;
  try {
    const extension = path.join(temp, 'extension');
    const folders = await mcpFolders(temp);
    const { workspace, cwd, home, scratch: envTemp } = folders;
    for (const [name, entry] of entries) {
      const file = path.resolve(extension, name);
      assert.ok(inside(extension, file), `Bundle path escapes extraction folder: ${name}`);
      await mkdir(path.dirname(file), { recursive: true });
      await writeFile(file, entry.read(), { flag: 'wx' });
    }
    const projectBefore = await footprint(base, tempRoot, new Set(['.git']));
    const targetBefore = await readdir(tempRoot);
    client = startMcp({ manifest, extension, ...folders });
    await client.initialize();
    const listed = await client.listTools();
    assert.equal(listed.length, manifest.tools.length);
    assert.deepEqual(new Set(listed.map(tool => tool.name)), new Set(manifest.tools.map(tool => tool.name)), 'Core catalog differs from manifest tools');
    const called = await client.request('tools/call', { name: 'cutbolt_run', arguments: { command: 'capabilities', arguments: {} } });
    assert.equal(called.isError, false, `Capabilities failed: ${JSON.stringify(called)}`);
    const content = called.structuredContent ?? JSON.parse(called.content.find(item => item.type === 'text').text);
    assert.equal(content.ok, true);
    assert.equal(content.result.local_only, true);
    assert.equal(path.resolve(content.result.workspace.root), workspace);
    await client.stop();
    for (const folder of [cwd, home, envTemp]) assert.deepEqual(await readdir(folder), [], `Server wrote outside workspace: ${folder}`);
    assert.deepEqual((await readdir(workspace)).sort(), ['.cutbolt']);
    assert.deepEqual((await readdir(path.join(workspace, '.cutbolt'))).sort(), ['cache', 'jobs', 'scratch', 'store']);
    for (const [name, entry] of entries) assert.equal(sha256(await readFile(path.join(extension, name))), sha256(entry.read()), `Server modified bundled ${name}`);
    assert.deepEqual((await footprint(extension)).map(([name]) => name).sort(), ['DEPENDENCIES.md', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'manifest.json', 'server', path.join('server', 'cutbolt.exe')].sort());
    assert.deepEqual(await footprint(base, tempRoot, new Set(['.git'])), projectBefore, 'Server wrote into the checkout outside temporary folders');
    assert.deepEqual((await readdir(tempRoot)).sort(), targetBefore.sort(), 'Server wrote beside its temporary folders');
    return { version: manifest.version, tools: listed.length, files: entries.size };
  } finally {
    try { await client?.stop(); } finally {
      assert.ok(inside(tempRoot, temp), 'Refusing to remove a folder outside target');
      await rm(temp, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    }
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2), allowSkip = args.includes('--allow-skip');
  const files = args.filter(arg => arg !== '--allow-skip');
  Promise.resolve().then(() => {
    if (files.length !== 1 || files[0].startsWith('--') || args.length > (allowSkip ? 2 : 1)) throw new Error('Usage: node mcpb/verify.mjs <bundle> [--allow-skip]');
    return verifyMcpb(files[0], { allowSkip });
  }).then(result => console.log(result.skipped ? result.message : `MCPB verified: Cutbolt ${result.version}, ${result.tools} tools, ${result.files} files; capabilities and write guards passed.`))
    .catch(error => { console.error(`MCPB verification failed: ${error.message}`); process.exitCode = 1; });
}
