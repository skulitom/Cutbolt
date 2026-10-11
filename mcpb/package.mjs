// Native CLI archives: executable, project documentation and dependency notices.
// Deterministic ustar/gzip adapted from skulitom/PixelForge's MIT build-studio.mjs.
import { readFile } from 'node:fs/promises';
import { deflateRawSync } from 'node:zlib';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { root, cargoVersion, readText, readNotices, writeUnchangedOrNew } from './build.mjs';
import { writeZip, crc32, sha256, checkName } from './zip.mjs';

export const TARGETS = ['x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu', 'aarch64-apple-darwin'];

export function writeTarGz(files, executable) {
  const blocks = [], octal = (value, length) => value.toString(8).padStart(length - 1, '0') + '\0';
  for (const [name, data] of [...files].sort(([a], [b]) => a < b ? -1 : 1)) {
    checkName(name);
    if (name.length > 100 || data.length >= 8 ** 11) throw new Error('Native archive exceeds ustar limits');
    const header = Buffer.alloc(512);
    header.write(name, 0, 'ascii'); header.write(octal(name === executable ? 0o755 : 0o644, 8), 100);
    header.write(octal(0, 8), 108); header.write(octal(0, 8), 116);
    header.write(octal(data.length, 12), 124); header.write(octal(315532800, 12), 136); // 1980-01-01
    header.fill(32, 148, 156); header.write('0', 156); header.write('ustar\0' + '00', 257, 'ascii');
    header.write(octal(header.reduce((sum, byte) => sum + byte, 0), 7) + ' ', 148);
    blocks.push(header, data, Buffer.alloc((512 - data.length % 512) % 512));
  }
  blocks.push(Buffer.alloc(1024));
  const tar = Buffer.concat(blocks), body = deflateRawSync(tar, { level: 9 }), trailer = Buffer.alloc(8);
  trailer.writeUInt32LE(crc32(tar), 0); trailer.writeUInt32LE(tar.length % 0x100000000, 4);
  return Buffer.concat([Buffer.from([0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 2, 3]), body, trailer]);
}

export async function packageBinary({ base = root, target, binary, notices, out = path.join(base, 'target/dist/release') }) {
  if (!TARGETS.includes(target)) throw new Error(`Unsupported archive target: ${target}`);
  const version = await cargoVersion(base), windows = target === TARGETS[0], name = windows ? 'cutbolt.exe' : 'cutbolt';
  const files = new Map([[name, await readFile(binary)]]);
  for (const file of ['LICENSE', 'README.md', 'docs/DEPENDENCIES.md']) files.set(file, await readText(path.join(base, file)));
  files.set('THIRD_PARTY_NOTICES.md', await readNotices(notices));
  const bytes = windows ? writeZip(files) : writeTarGz(files, name);
  const archive = path.resolve(out, `cutbolt-v${version}-${target}.${windows ? 'zip' : 'tar.gz'}`);
  await writeUnchangedOrNew(archive, bytes);
  return { archive, hash: sha256(bytes) };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  Promise.resolve().then(() => {
    const positional = [], options = {}, argv = process.argv.slice(2);
    for (let i = 0; i < argv.length; i++) {
      if (argv[i] === '--notices' && !options.notices && argv[i + 1] && !argv[i + 1].startsWith('--')) options.notices = argv[++i];
      else if (argv[i].startsWith('--')) throw new Error(`Unknown or incomplete option: ${argv[i]}`);
      else positional.push(argv[i]);
    }
    const [target, binary, out] = positional;
    if (!target || !binary || positional.length > 3) throw new Error('Usage: node mcpb/package.mjs <target> <binary> [out] --notices <file>');
    return packageBinary({ target, binary, out, notices: options.notices });
  }).then(result => console.log(`${result.archive}\nSHA-256 ${result.hash}`)).catch(error => { console.error(error.message); process.exitCode = 1; });
}
