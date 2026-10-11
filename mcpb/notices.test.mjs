import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import path from 'node:path';
import { normalPackages, renderNotices } from './notices.mjs';

// Entirely synthetic, in-memory Cargo metadata and file reader.
function fixture() {
  const names = ['cutbolt', 'alpha', 'alpha', 'zeta', 'dev-only', 'build-only', 'dev-child', 'build-child', 'other-platform'];
  const packages = names.map((name, i) => ({
    id: `pkg-${i}`, name, version: i === 2 ? '1.0.0' : '2.0.0', license: 'MIT OR Apache-2.0', license_file: null,
    manifest_path: path.resolve('synthetic-cargo', `pkg-${i}`, 'Cargo.toml')
  }));
  const dep = (i, ...kinds) => ({ name: names[i], pkg: `pkg-${i}`, dep_kinds: kinds.map(kind => ({ kind, target: null })) });
  const nodes = packages.map(pkg => ({ id: pkg.id, deps: [] }));
  nodes[0].deps = [dep(1, null), dep(4, 'dev'), dep(5, 'build')];
  nodes[1].deps = [dep(3, 'build', null), dep(2, null), dep(5, 'build')];
  nodes[2].deps = [dep(3, null)]; // Shared transitive dependency; count only once.
  nodes[3].deps = [dep(1, null)]; // Traversal also terminates on cycles.
  nodes[4].deps = [dep(6, null)];
  nodes[5].deps = [dep(7, null)];
  // Unreachable packages can remain in Cargo metadata after platform filtering.
  nodes[1].deps[0].dep_kinds[1].target = 'cfg(target_os = "synthetic-os")';
  const metadata = { packages, resolve: { root: 'pkg-0', nodes } };
  const files = new Map(), directories = new Map(), reads = [];
  for (const pkg of packages.slice(1, 4)) directories.set(path.dirname(pkg.manifest_path), []);
  const add = (i, name, text, file = true) => {
    const directory = path.dirname(packages[i].manifest_path);
    if (!name.includes('/')) directories.get(directory).push({ name, isFile: () => file });
    if (file) files.set(path.resolve(directory, name), text);
  };
  add(1, 'LICENSE-MIT', 'Shared synthetic terms\r\nKeep this line\r\n \t');
  add(2, 'LICENSE', 'Shared synthetic terms\nKeep this line\n');
  add(3, 'COPYING', 'Different synthetic terms\n');
  const reader = {
    readdir: async directory => {
      assert.ok(directories.has(directory), `Must not read excluded package ${directory}`);
      return directories.get(directory);
    },
    readFile: async file => {
      reads.push(file);
      if (!files.has(file)) throw new Error(`No fixture file: ${file}`);
      return files.get(file);
    }
  };
  return { metadata, packages, files, directories, reads, add, reader };
}

test('only normal reachable dependencies are included; Cargo owns platform filtering', async () => {
  const f = fixture();
  assert.deepEqual(normalPackages(f.metadata).map(pkg => `${pkg.name} ${pkg.version}`), ['alpha 1.0.0', 'alpha 2.0.0', 'zeta 2.0.0']);
  const result = await renderNotices(f.metadata, f.reader);
  assert.equal(result.packageCount, 3);
  assert.equal(f.reads.length, 3);
  for (const name of ['cutbolt', 'dev-only', 'build-only', 'dev-child', 'build-child', 'other-platform']) {
    assert.ok(!result.text.includes(`| ${name} |`));
  }
});

test('license_file and every matching top-level file are included, without walking subdirectories', async () => {
  const f = fixture();
  f.packages[1].license_file = 'legal/terms.txt';
  f.add(1, 'legal/terms.txt', 'Explicit synthetic terms');
  const accepted = ['LICENCE', 'license.txt', 'LiCeNsE-Zlib', 'license_custom', 'NOTICE', 'NOTICE.txt', 'COPYING-extra', 'Copyright', 'copyright_more', 'UNLICENSE', 'unlicense.txt'];
  for (const name of accepted) f.add(1, name, `Synthetic text for ${name}`);
  for (const name of ['LICENSES', 'licensed.txt', 'notices.md', 'README', 'my-LICENSE', 'legal/NOTICE']) f.add(1, name, 'Must not include');
  f.add(1, 'LICENSE-directory', undefined, false);
  const result = await renderNotices(f.metadata, f.reader);
  for (const name of ['legal/terms.txt', 'LICENSE-MIT', ...accepted]) assert.ok(result.text.includes(`${name}: [notice-`), name);
  assert.ok(!result.text.includes('Must not include'));
  assert.equal(f.reads.length, 4 + accepted.length);
});

test('shared normalized texts have one stable SHA-256 section and per-file references', async () => {
  const f = fixture();
  f.packages[2].license_file = 'LICENSE'; // Same file as the top-level scan.
  const result = await renderNotices(f.metadata, f.reader);
  const text = 'Shared synthetic terms\nKeep this line';
  const id = `notice-${createHash('sha256').update(text).digest('hex').slice(0, 16)}`;
  assert.equal(result.noticeCount, 2);
  assert.equal(result.text.split(`## ${id}\n`).length - 1, 1);
  assert.equal(result.text.split(`[${id}](#${id})`).length - 1, 2);
  assert.ok(result.text.includes('```text\n' + text + '\n```'));
  assert.ok(!result.text.includes('\r'));
  assert.equal(f.reads.length, 3);
});

test('rendering is deterministic across package, graph and directory ordering', async () => {
  const f = fixture();
  f.add(1, 'NOTICE', 'Another synthetic notice');
  const before = JSON.stringify(f.metadata), first = await renderNotices(f.metadata, f.reader);
  assert.equal(JSON.stringify(f.metadata), before, 'Renderer must not mutate metadata');
  f.metadata.packages.reverse();
  f.metadata.resolve.nodes.reverse();
  for (const node of f.metadata.resolve.nodes) {
    node.deps.reverse();
    for (const dep of node.deps) dep.dep_kinds.reverse();
  }
  for (const entries of f.directories.values()) entries.reverse();
  assert.deepEqual(await renderNotices(f.metadata, f.reader), first);
});

test('missing and whitespace-only license texts fail closed with all affected packages', async () => {
  const f = fixture();
  f.directories.get(path.dirname(f.packages[1].manifest_path)).length = 0;
  f.files.set(path.resolve(path.dirname(f.packages[3].manifest_path), 'COPYING'), '\r\n \t');
  await assert.rejects(renderNotices(f.metadata, f.reader), error => {
    assert.match(error.message, /No license text for: alpha 2.0.0, zeta 2.0.0/);
    assert.deepEqual(error.packages, ['alpha 2.0.0', 'zeta 2.0.0']);
    assert.equal(error.packageCount, 3);
    return true;
  });
});

test('an unreadable explicit license file fails even when other texts exist', async () => {
  const f = fixture();
  f.packages[1].license_file = 'absent.txt';
  await assert.rejects(renderNotices(f.metadata, f.reader), /Cannot read notices for alpha 2.0.0: No fixture file/);
});

test('invalid roots and incomplete reachable graphs fail closed', async () => {
  const f = fixture();
  assert.throws(() => normalPackages({ ...f.metadata, resolve: null }), /root package cutbolt/);
  f.metadata.resolve.root = 'pkg-1';
  assert.throws(() => normalPackages(f.metadata), /root package cutbolt/);
  f.metadata.resolve.root = 'pkg-0';
  f.metadata.resolve.nodes = f.metadata.resolve.nodes.filter(node => node.id !== 'pkg-3');
  await assert.rejects(renderNotices(f.metadata, f.reader), /Incomplete Cargo dependency graph: pkg-3/);
});

test('notice contents keep internal whitespace and Markdown fences intact', async () => {
  const f = fixture(), text = 'Synthetic terms  \r\n```text\rSecond line\r\n```\r\n\t';
  f.files.set(path.resolve(path.dirname(f.packages[3].manifest_path), 'COPYING'), text);
  const result = await renderNotices(f.metadata, f.reader);
  assert.ok(result.text.includes('````text\nSynthetic terms  \n```text\nSecond line\n```\n````'));
});
