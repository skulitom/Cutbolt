import test, { before, after } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFile, writeFile, mkdir, mkdtemp, rm, stat, link, readdir } from 'node:fs/promises';
import { gunzipSync } from 'node:zlib';
import path from 'node:path';
import { root, BUNDLE, cargoVersion, json, readJSON, assembleMcpb, buildMcpb, registryEntry, mergeTools, discoverTools } from './build.mjs';
import { readZip, writeZip, crc32, sha256 } from './zip.mjs';
import { checkManifest, verifyMcpb } from './verify.mjs';
import { packageBinary, TARGETS } from './package.mjs';

let temp, base, binary, notices, build, template, metadata, fixtureTools;
// Packaging fixtures need no executable; the real-binary test covers discovery.
const assembleFixture = options => assembleMcpb(options, async () => fixtureTools);
const buildFixture = options => buildMcpb(options, async () => fixtureTools);
const version = '7.8.9';
const writeJSON = (file, value) => writeFile(file, json(value));
const writeCargo = value => writeFile(path.join(base, 'Cargo.toml'), `[package]\nname = "fixture"\nversion = "${value}"\n\n[dependencies]\n`);
before(async () => {
  await mkdir(path.join(root, 'target'), { recursive: true });
  temp = await mkdtemp(path.join(root, 'target/cutbolt-distribution-test-'));
  base = path.join(temp, 'source');
  await mkdir(path.join(base, 'mcpb'), { recursive: true });
  await mkdir(path.join(base, 'docs'));
  template = await readJSON(path.join(root, 'mcpb/manifest.json'));
  fixtureTools = [...template.tools.slice(1).reverse(), { name: 'cutbolt_new', description: 'New tool sentence. More detail.' }];
  metadata = { ...await readJSON(path.join(root, 'server.json')), version: '0.0.1' };
  await writeJSON(path.join(base, 'mcpb/manifest.json'), template);
  await writeJSON(path.join(base, 'server.json'), metadata);
  await writeCargo(version);
  for (const name of ['LICENSE', 'README.md', 'docs/DEPENDENCIES.md', 'THIRD_PARTY_NOTICES.md']) await writeFile(path.join(base, name), `Synthetic fixture ${name}\nSecond line\n`);
  notices = path.join(base, 'THIRD_PARTY_NOTICES.md');
  binary = path.join(base, 'stand-in.exe');
  await writeFile(binary, Buffer.from([77, 90, 0, 13, 10, 255, 0, 1]));
  build = await buildFixture({ base, binary, notices, out: path.join(temp, 'first') });
});
after(async () => {
  if (!temp) return;
  assert.equal(path.dirname(temp), path.join(root, 'target'));
  await rm(temp, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
});

test('source metadata is short and metadata-only; manifest template is valid and versionless', async () => {
  const source = await readJSON(path.join(root, 'server.json'));
  assert.equal(source.$schema, 'https://static.modelcontextprotocol.io/schemas/2025-12-11/server.schema.json');
  assert.equal(source.name, 'io.github.skulitom/cutbolt');
  assert.equal(source.repository.id, '1404146840');
  assert.ok(source.description.length > 0 && source.description.length <= 100);
  assert.equal(Object.hasOwn(source, 'packages'), false);
  assert.equal(Object.hasOwn(template, 'version'), false);
  checkManifest({ ...template, version: await cargoVersion() }, await cargoVersion());
});

test('tool merge keeps curated descriptions, drops absent tools and follows live order without mutation', () => {
  const overrides = [
    { name: 'cutbolt_first', description: 'Curated first.' },
    { name: 'cutbolt_dropped', description: 'Retired.' },
    { name: 'cutbolt_last', description: 'Curated last.' }
  ];
  const live = [
    { name: 'cutbolt_last', description: 'Server last.', inputSchema: {} },
    { name: 'cutbolt_first', description: 'Server first.' }
  ];
  const before = structuredClone({ overrides, live });
  assert.deepEqual(mergeTools(overrides, live), {
    tools: [overrides[2], overrides[0]], added: [], dropped: ['cutbolt_dropped']
  });
  assert.deepEqual({ overrides, live }, before);
});

test('new tool descriptions use the first sentence and normalize whitespace', () => {
  for (const [description, expected] of [
    ['  Make a\nsmall image. Keep the source.  ', 'Make a small image.'],
    ['Inspect it! Then edit.', 'Inspect it!'],
    ['Ready? More details.', 'Ready?'],
    ['Render at 1.5 scale. More details.', 'Render at 1.5 scale.'],
    ['  No final punctuation  ', 'No final punctuation']
  ]) {
    assert.deepEqual(mergeTools([], [{ name: 'cutbolt_new', description }]), {
      tools: [{ name: 'cutbolt_new', description: expected }], added: ['cutbolt_new'], dropped: []
    });
  }
});

test('new tool descriptions stop at a whole word within 160 characters', () => {
  const merge = description => mergeTools([], [{ name: 'cutbolt_new', description }]).tools[0].description;
  const exact = 'word '.repeat(31) + 'final';
  assert.equal(exact.length, 160);
  assert.equal(merge(exact), exact);
  assert.equal(merge(exact + ' more words.'), exact);
  assert.equal(merge('word '.repeat(31) + 'longerword.'), 'word '.repeat(31).trim());
  assert.throws(() => merge('x'.repeat(161)), /word boundary/);
});

test('tool merge rejects empty catalogs, invalid names and duplicates', () => {
  for (const live of [[], undefined, {}]) assert.throws(() => mergeTools([], live), /no tools/);
  for (const name of ['', 'cutbolt_', 'other_tool', 'cutbolt_bad-name', 'cutbolt_bad.name', 'cutbolt_é', 'cutbolt_bad\n', ' cutbolt_bad', 7, null, undefined]) {
    assert.throws(() => mergeTools([], [{ name, description: 'Description.' }]), /Invalid tool name/);
  }
  assert.throws(() => mergeTools([], [null]), /Invalid tool name/);
  const tool = { name: 'cutbolt_valid_123', description: 'Description.' };
  assert.deepEqual(mergeTools([], [tool]).tools, [tool]);
  assert.throws(() => mergeTools([], [tool, tool]), /Duplicate tool name/);
});

test('new tools require usable descriptions while curated descriptions remain exact', () => {
  for (const description of [undefined, null, '', ' \n ']) {
    assert.throws(() => mergeTools([], [{ name: 'cutbolt_new', description }]), /server description/);
  }
  const override = { name: 'cutbolt_known', description: 'Curated text. A second sentence is retained.' };
  assert.deepEqual(mergeTools([override], [{ name: override.name }]).tools, [override]);
});

test('two builds have the exact entries, binary bytes, checksum and deterministic LF text', async () => {
  const second = await buildFixture({ base, binary, notices, out: path.join(temp, 'second') });
  assert.deepEqual(second.bytes, build.bytes);
  const entries = readZip(build.bytes);
  assert.deepEqual([...entries.keys()], ['DEPENDENCIES.md', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'manifest.json', 'server/cutbolt.exe']);
  assert.deepEqual(entries.get('server/cutbolt.exe').read(), await readFile(binary));
  assert.deepEqual(entries.get('THIRD_PARTY_NOTICES.md').read(), await readFile(notices));
  const generated = JSON.parse(entries.get('manifest.json').read());
  assert.equal(generated.version, version);
  assert.deepEqual(generated.tools, mergeTools(template.tools, fixtureTools).tools);
  assert.deepEqual(build.added, ['cutbolt_new']);
  assert.deepEqual(build.dropped, [template.tools[0].name]);
  assert.equal(await readFile(`${build.bundle}.sha256`, 'utf8'), `${sha256(build.bytes)}  ${BUNDLE}\n`);
  for (const name of ['LICENSE', 'docs/DEPENDENCIES.md', 'mcpb/manifest.json', 'THIRD_PARTY_NOTICES.md']) {
    const file = path.join(base, name), original = await readFile(file, 'utf8');
    try { await writeFile(file, original.replaceAll('\n', '\r\n')); assert.deepEqual((await assembleFixture({ base, binary, notices })).bytes, build.bytes); }
    finally { await writeFile(file, original); }
  }
});

test('builder rejects template versions, invalid Cargo versions, missing binary and changed existing output', async () => {
  try {
    await writeJSON(path.join(base, 'mcpb/manifest.json'), { ...template, version });
    await assert.rejects(assembleFixture({ base, binary, notices }), /must not contain version/);
  } finally { await writeJSON(path.join(base, 'mcpb/manifest.json'), template); }
  try {
    for (const value of ['1.0', 'v1.2.3', '01.2.3', '1.2.3-beta', '1.2.3+build']) {
      await writeCargo(value);
      await assert.rejects(assembleFixture({ base, binary, notices }), /version must have the form X.Y.Z/);
    }
  } finally { await writeCargo(version); }
  await assert.rejects(assembleFixture({ base, binary: path.join(base, 'absent.exe'), notices }), /Missing binary/);
  const out = path.join(temp, 'occupied'); await mkdir(out); await writeFile(path.join(out, BUNDLE), 'keep');
  await assert.rejects(buildFixture({ base, binary, notices, out }), /already exists with different contents/);
  assert.equal(await readFile(path.join(out, BUNDLE), 'utf8'), 'keep');
});

test('registry entry stamps Cargo version despite stale metadata and hashes the exact bundle', async () => {
  const before = await readFile(path.join(base, 'server.json')), out = path.join(temp, 'server.registry.json');
  const options = { base, tag: `v${version}`, bundle: build.bundle, out };
  const entry = await registryEntry(options);
  assert.equal(entry.version, version);
  assert.deepEqual(entry.packages, [{ registryType: 'mcpb', identifier: `https://github.com/skulitom/Cutbolt/releases/download/v${version}/${BUNDLE}`, version, fileSha256: sha256(build.bytes), transport: { type: 'stdio' } }]);
  assert.deepEqual(await readJSON(out), entry);
  assert.deepEqual(await readFile(path.join(base, 'server.json')), before);
  await registryEntry(options);
});

test('a Cargo version bump requires no distribution metadata edits', async () => {
  try {
    await writeCargo('12.34.56');
    const newer = await buildFixture({ base, binary, notices, out: path.join(temp, 'bumped') });
    checkManifest(newer.manifest, '12.34.56');
    const entry = await registryEntry({ base, tag: 'v12.34.56', bundle: newer.bundle, out: path.join(temp, 'bumped-entry.json') });
    assert.equal(entry.version, '12.34.56');
    assert.equal((await readJSON(path.join(base, 'server.json'))).version, metadata.version);
  } finally { await writeCargo(version); }
});

test('registry entry rejects malformed tags, tag mismatch, packages and a mismatched bundle', async () => {
  const options = { base, tag: `v${version}`, bundle: build.bundle, out: path.join(temp, 'refused.json') };
  for (const tag of ['7.8.9', 'v1.2', 'v01.2.3', 'v1.2.3-beta', '../v1.2.3', 'v1.2.3\n']) {
    await assert.rejects(registryEntry({ ...options, tag }), /Tag must have the form/);
  }
  await assert.rejects(registryEntry({ ...options, tag: 'v9.9.9' }), /versions must match/);
  try {
    await writeJSON(path.join(base, 'server.json'), { ...metadata, packages: [] });
    await assert.rejects(registryEntry(options), /metadata-only/);
  } finally { await writeJSON(path.join(base, 'server.json'), metadata); }
  const files = new Map([...readZip(build.bytes)].map(([name, entry]) => [name, entry.read()]));
  files.set('manifest.json', json({ ...build.manifest, version: '6.0.0' }));
  const old = path.join(temp, 'old.mcpb'); await writeFile(old, writeZip(files));
  await assert.rejects(registryEntry({ ...options, bundle: old }), /Bundle manifest version must match/);
});

test('registry entry refuses source, bundle and hard-link output collisions', async () => {
  const alias = path.join(temp, 'alias.json');
  await link(path.join(base, 'server.json'), alias);
  for (const out of [path.join(base, 'server.json'), build.bundle, alias]) {
    const before = await readFile(out);
    await assert.rejects(registryEntry({ base, tag: `v${version}`, bundle: build.bundle, out }), /must not overwrite/);
    assert.deepEqual(await readFile(out), before);
  }
});

test('ZIP reader checks CRCs and rejects malformed or unsafe entries', () => {
  assert.equal(crc32(Buffer.from('123456789')), 0xcbf43926);
  assert.throws(() => readZip(Buffer.from('broken')), /Not a ZIP archive/);
  for (const name of ['../escape', '/absolute', 'C:/drive', 'bad\\separator', 'file:stream', 'NUL', 'trailing.']) {
    assert.throws(() => writeZip(new Map([[name, Buffer.from('x')]])), /Unsafe ZIP/);
  }
  const bytes = writeZip(new Map([['file', Buffer.from([1, 2, 3])]]));
  bytes[34] ^= 1;
  assert.throws(() => readZip(bytes).get('file').read(), /damaged/);
  assert.throws(() => writeZip(new Map([['a', Buffer.from('x')], ['A', Buffer.from('y')]])), /Duplicate ZIP/);
});

test('native archives contain binary and notices, and tar preserves execute permission deterministically', async () => {
  for (const target of TARGETS) {
    const first = await packageBinary({ base, binary, notices, target, out: path.join(temp, 'archives') });
    const second = await packageBinary({ base, binary, notices, target, out: path.join(temp, 'archives-again') });
    assert.equal(first.hash, second.hash);
    const bytes = await readFile(first.archive);
    if (target.includes('windows')) {
      const entries = readZip(bytes);
      assert.deepEqual([...entries.keys()], ['LICENSE', 'README.md', 'THIRD_PARTY_NOTICES.md', 'cutbolt.exe', 'docs/DEPENDENCIES.md']);
      assert.deepEqual(entries.get('THIRD_PARTY_NOTICES.md').read(), await readFile(notices));
    } else {
      const tar = gunzipSync(bytes), files = new Map();
      for (let at = 0; tar[at];) {
        const name = tar.toString('ascii', at, at + 100).split('\0')[0];
        const size = parseInt(tar.toString('ascii', at + 124, at + 136), 8);
        const header = Buffer.from(tar.subarray(at, at + 512));
        const checksum = parseInt(header.toString('ascii', 148, 156), 8); header.fill(32, 148, 156);
        assert.equal(checksum, header.reduce((sum, byte) => sum + byte, 0));
        files.set(name, { mode: parseInt(tar.toString('ascii', at + 100, at + 108), 8), data: tar.subarray(at + 512, at + 512 + size) });
        at += 512 + Math.ceil(size / 512) * 512;
      }
      assert.deepEqual([...files.keys()], ['LICENSE', 'README.md', 'THIRD_PARTY_NOTICES.md', 'cutbolt', 'docs/DEPENDENCIES.md']);
      assert.equal(files.get('cutbolt').mode, 0o755);
      assert.deepEqual(files.get('cutbolt').data, await readFile(binary));
      assert.deepEqual(files.get('THIRD_PARTY_NOTICES.md').data, await readFile(notices));
    }
  }
});

test('bundle and archive builders require a readable, nonempty notices file', async () => {
  const empty = path.join(temp, 'empty-notices.md');
  await writeFile(empty, ' \r\n\t');
  for (const create of [assembleFixture, packageBinary]) {
    const options = { base, binary, target: TARGETS[0], out: path.join(temp, 'missing-notices') };
    await assert.rejects(create(options), /--notices <file> is required/);
    await assert.rejects(create({ ...options, notices: empty }), /Empty notices file/);
    await assert.rejects(create({ ...options, notices: path.join(temp, 'absent.md') }), /ENOENT/);
  }
});

test('bundle and archive CLIs accept --notices and reject its omission before discovery', async () => {
  const run = (script, args) => execFileSync(process.execPath, [path.join(root, 'mcpb', script), ...args], {
    cwd: temp, encoding: 'utf8', windowsHide: true, stdio: ['ignore', 'pipe', 'pipe']
  });
  for (const [script, args, filename] of [
    ['build.mjs', ['--binary', binary, '--out', path.join(temp, 'cli-bundle')], BUNDLE],
    ['package.mjs', [TARGETS[0], binary, path.join(temp, 'cli-archive')], `cutbolt-v${await cargoVersion()}-${TARGETS[0]}.zip`]
  ]) {
    assert.throws(() => run(script, args), error => error.status === 1 && /--notices <file> is required/.test(error.stderr));
    if (script === 'build.mjs') {
      // The stand-in reaches discovery only after --notices is accepted and read.
      // Successful bundle CLI packaging is covered by the real-binary test below.
      assert.throws(() => run(script, [...args, '--notices', notices]), error => error.status === 1 && /spawn|exited|UNKNOWN|EINVAL/.test(error.stderr));
      continue;
    }
    run(script, [...args, '--notices', notices]);
    const out = args.at(-1);
    assert.deepEqual(readZip(await readFile(path.join(out, filename))).get('THIRD_PARTY_NOTICES.md').read(), await readFile(notices));
  }
});

test('discovery cleans up when a stand-in executable cannot start', async () => {
  await assert.rejects(discoverTools({ manifest: { ...template, version }, binary, base }), /spawn|exited|UNKNOWN|EINVAL/);
  assert.deepEqual(await readdir(path.join(base, 'target')), []);
});

test('verifier rejects manifest drift before starting a server', () => {
  for (const overrides of [{ version: '0.0.0' }, { compatibility: { platforms: ['linux'] } }, { server: { ...build.manifest.server, mcp_config: { command: 'untrusted.exe' } } }]) {
    assert.throws(() => checkManifest({ ...build.manifest, ...overrides }, version));
  }
});

test('verifier cleans up when a stand-in executable cannot start', { skip: process.platform !== 'win32' }, async () => {
  await assert.rejects(verifyMcpb(build.bundle, { base }), /spawn|exited|UNKNOWN|EINVAL/);
  assert.deepEqual(await readdir(path.join(base, 'target')), []);
});

test('non-Windows verifier requires explicit --allow-skip', { skip: process.platform === 'win32' }, async () => {
  await assert.rejects(verifyMcpb(build.bundle), /Windows only/);
  assert.equal((await verifyMcpb(build.bundle, { allowSkip: true })).skipped, true);
});

test('real release binary passes the standalone bundle verifier', async t => {
  const realBinary = path.join(root, 'target/release/cutbolt.exe');
  if (process.platform !== 'win32' || !await stat(realBinary).catch(e => { if (e.code !== 'ENOENT') throw e; return null; })) {
    t.skip('Windows release binary is not available'); return;
  }
  // A stale local executable must not break metadata tests after Cargo.toml is bumped.
  // CI always rebuilds first, so it exercises the matching binary.
  const actual = execFileSync(realBinary, ['--version'], { encoding: 'utf8', timeout: 5000, windowsHide: true });
  if (!actual.startsWith(`cutbolt ${await cargoVersion()} `) && actual.trim() !== `cutbolt ${await cargoVersion()}`) {
    t.skip('Local release binary predates Cargo.toml; rebuild to test it'); return;
  }
  const real = await buildMcpb({ binary: realBinary, notices, out: path.join(temp, 'real') });
  execFileSync(process.execPath, [path.join(root, 'mcpb/build.mjs'), '--binary', realBinary, '--notices', notices, '--out', path.dirname(real.bundle)], {
    encoding: 'utf8', timeout: 20000, windowsHide: true
  });
  assert.deepEqual(readZip(await readFile(real.bundle)).get('THIRD_PARTY_NOTICES.md').read(), await readFile(notices));
  const output = execFileSync(process.execPath, [path.join(root, 'mcpb/verify.mjs'), real.bundle], { encoding: 'utf8', timeout: 20000, windowsHide: true });
  assert.ok(output.includes(`MCPB verified: Cutbolt ${real.manifest.version}, ${real.manifest.tools.length} tools, 5 files; capabilities and write guards passed.`));
  const changed = new Map([...readZip(real.bytes)].map(([name, entry]) => [name, entry.read()]));
  changed.set('manifest.json', json({ ...real.manifest, tools: [
    ...real.manifest.tools.slice(1), { name: 'cutbolt_not_in_binary', description: 'Synthetic mismatch.' }
  ] }));
  const mismatched = path.join(temp, 'mismatched.mcpb');
  await writeFile(mismatched, writeZip(changed));
  await assert.rejects(verifyMcpb(mismatched), /Core catalog differs from manifest tools/);
});
