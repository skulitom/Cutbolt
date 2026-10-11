// Smoke-test the extracted binary with its manifest command and an existing, empty workspace.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdir, mkdtemp, readdir, lstat, rm } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { root, cargoVersion, VERSION } from './build.mjs';
import { readZip, sha256 } from './zip.mjs';

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

const inside = (base, file) => { const rel = path.relative(base, file); return rel && rel !== '..' && !rel.startsWith(`..${path.sep}`) && !path.isAbsolute(rel); };

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
  let child, closed, timer;
  try {
    const extension = path.join(temp, 'extension'), workspace = path.join(temp, 'workspace'), cwd = path.join(temp, 'cwd');
    const home = path.join(temp, 'home'), envTemp = path.join(temp, 'tmp');
    for (const folder of [workspace, cwd, home, envTemp]) await mkdir(folder);
    for (const [name, entry] of entries) {
      const file = path.resolve(extension, name);
      assert.ok(inside(extension, file), `Bundle path escapes extraction folder: ${name}`);
      await mkdir(path.dirname(file), { recursive: true });
      await writeFile(file, entry.read(), { flag: 'wx' });
    }
    const projectBefore = await footprint(base, tempRoot, new Set(['.git']));
    const targetBefore = await readdir(tempRoot);
    const substitute = value => {
      const result = value.replaceAll('${__dirname}', extension).replaceAll('${user_config.workspace}', workspace);
      assert.ok(!result.includes('${'), `Unresolved MCP variable: ${result}`);
      return result;
    };
    const config = manifest.server.mcp_config, pending = new Map();
    let id = 0, buffer = '', stderr = '', failure;
    const fail = error => { failure ??= error; for (const request of pending.values()) request.reject(error); pending.clear(); };
    child = spawn(substitute(config.command), config.args.map(substitute), {
      cwd, env: { ...process.env, HOME: home, USERPROFILE: home, APPDATA: home, LOCALAPPDATA: home, TEMP: envTemp, TMP: envTemp,
        // Prove that this one harmless call works without external media executables.
        CUTBOLT_FFMPEG: path.join(temp, 'missing-ffmpeg.exe'), CUTBOLT_FFPROBE: path.join(temp, 'missing-ffprobe.exe') },
      stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true
    });
    closed = new Promise(resolve => child.once('close', (code, signal) => { fail(new Error(`MCP server exited (${code ?? signal}): ${stderr}`)); resolve(); }));
    child.on('error', fail);
    child.stdin.on('error', fail);
    child.stderr.setEncoding('utf8'); child.stderr.on('data', data => { stderr = (stderr + data).slice(-8000); });
    child.stdout.setEncoding('utf8');
    child.stdout.on('data', data => {
      try {
        buffer += data;
        assert.ok(buffer.length < 4 * 1024 * 1024, 'MCP response exceeds smoke-test budget');
        let end;
        while ((end = buffer.indexOf('\n')) !== -1) {
          const line = buffer.slice(0, end); buffer = buffer.slice(end + 1);
          const message = JSON.parse(line), request = pending.get(message.id);
          assert.equal(message.jsonrpc, '2.0');
          if (!request) continue;
          pending.delete(message.id);
          if (message.error) request.reject(new Error(`${request.method}: ${JSON.stringify(message.error)}`));
          else request.resolve(message.result);
        }
      } catch (error) { fail(error); }
    });
    timer = setTimeout(() => { fail(new Error(`MCP smoke test timed out after 12 seconds. ${stderr}`)); child.kill(); }, 12000);
    const send = message => child.stdin.write(JSON.stringify({ jsonrpc: '2.0', ...message }) + '\n');
    const request = (method, params) => new Promise((resolve, reject) => {
      if (failure) { reject(failure); return; }
      const current = ++id; pending.set(current, { resolve, reject, method }); send({ id: current, method, params });
    });
    const initialized = await request('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'cutbolt-mcpb-smoke', version: '1.0.0' } });
    assert.equal(initialized.serverInfo.name, 'cutbolt');
    assert.equal(initialized.serverInfo.version, manifest.version);
    assert.equal(initialized.protocolVersion, '2025-11-25');
    send({ method: 'notifications/initialized' });
    const listed = await request('tools/list', {});
    assert.equal(listed.tools.length, manifest.tools.length);
    assert.deepEqual(new Set(listed.tools.map(tool => tool.name)), new Set(manifest.tools.map(tool => tool.name)), 'Core catalog differs from manifest tools');
    const called = await request('tools/call', { name: 'cutbolt_capabilities', arguments: {} });
    assert.equal(called.isError, false, `Capabilities failed: ${JSON.stringify(called)}`);
    const content = called.structuredContent ?? JSON.parse(called.content.find(item => item.type === 'text').text);
    assert.equal(content.ok, true);
    assert.equal(content.result.local_only, true);
    assert.equal(path.resolve(content.result.workspace.root), workspace);
    child.kill(); await closed; clearTimeout(timer);
    for (const folder of [cwd, home, envTemp]) assert.deepEqual(await readdir(folder), [], `Server wrote outside workspace: ${folder}`);
    assert.deepEqual((await readdir(workspace)).sort(), ['.cutbolt']);
    assert.deepEqual((await readdir(path.join(workspace, '.cutbolt'))).sort(), ['cache', 'jobs', 'scratch', 'store']);
    for (const [name, entry] of entries) assert.equal(sha256(await readFile(path.join(extension, name))), sha256(entry.read()), `Server modified bundled ${name}`);
    assert.deepEqual((await footprint(extension)).map(([name]) => name).sort(), ['DEPENDENCIES.md', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'manifest.json', 'server', path.join('server', 'cutbolt.exe')].sort());
    assert.deepEqual(await footprint(base, tempRoot, new Set(['.git'])), projectBefore, 'Server wrote into the checkout outside temporary folders');
    assert.deepEqual((await readdir(tempRoot)).sort(), targetBefore.sort(), 'Server wrote beside its temporary folders');
    return { version: manifest.version, tools: listed.tools.length, files: entries.size };
  } finally {
    clearTimeout(timer);
    if (child) { child.kill(); await closed; }
    assert.ok(inside(tempRoot, temp), 'Refusing to remove a folder outside target');
    await rm(temp, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
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
