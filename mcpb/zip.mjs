// ZIP helpers adapted from skulitom/PixelForge's MIT build-studio.mjs.
// ZIP32 only; fixed dates, sorted names and no host metadata. No external packages.
import { createHash } from 'node:crypto';
import { deflateRawSync, inflateRawSync } from 'node:zlib';

export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
// IEEE CRC-32, including Node 20 where node:zlib does not yet expose crc32.
const table = Uint32Array.from({ length: 256 }, (_, n) => {
  for (let bit = 0; bit < 8; bit++) n = (n >>> 1) ^ (n & 1 ? 0xedb88320 : 0);
  return n >>> 0;
});
export function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) crc = table[(crc ^ byte) & 255] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

export function checkName(name) {
  if (!/^[\x20-\x7e]+$/.test(name) || /[\\:*?"<>|]/.test(name) ||
      name.split('/').some(p => !p || p === '.' || p === '..' || /[. ]$/.test(p) || /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i.test(p))) {
    throw new Error(`Unsafe ZIP entry name: ${name}`);
  }
}

export function writeZip(files) {
  if (files.size >= 65535) throw new Error('Too many ZIP32 entries');
  const local = [], central = [], names = new Set();
  let offset = 0;
  for (const [filename, data] of [...files].sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)) {
    checkName(filename);
    if (names.has(filename.toLowerCase())) throw new Error(`Duplicate ZIP entry: ${filename}`);
    names.add(filename.toLowerCase());
    const deflated = deflateRawSync(data, { level: 9 });
    const method = deflated.length < data.length ? 8 : 0, body = method ? deflated : data;
    const name = Buffer.from(filename), crc = crc32(data), header = Buffer.alloc(30), entry = Buffer.alloc(46);
    if (name.length > 65535 || offset + 30 + name.length + body.length > 0xffffffff) throw new Error('Archive exceeds ZIP32 limits');
    header.writeUInt32LE(0x04034b50, 0); header.writeUInt16LE(20, 4); header.writeUInt16LE(method, 8);
    header.writeUInt16LE(33, 12); // 1980-01-01, midnight UTC
    header.writeUInt32LE(crc, 14); header.writeUInt32LE(body.length, 18); header.writeUInt32LE(data.length, 22); header.writeUInt16LE(name.length, 26);
    entry.writeUInt32LE(0x02014b50, 0); entry.writeUInt16LE(20, 4); entry.writeUInt16LE(20, 6); entry.writeUInt16LE(method, 10); entry.writeUInt16LE(33, 14);
    entry.writeUInt32LE(crc, 16); entry.writeUInt32LE(body.length, 20); entry.writeUInt32LE(data.length, 24); entry.writeUInt16LE(name.length, 28); entry.writeUInt32LE(offset, 42);
    local.push(header, name, body); central.push(entry, name); offset += header.length + name.length + body.length;
  }
  const directory = Buffer.concat(central), end = Buffer.alloc(22);
  if (offset + directory.length + 22 > 0xffffffff) throw new Error('Archive exceeds ZIP32 limits');
  end.writeUInt32LE(0x06054b50, 0); end.writeUInt16LE(files.size, 8); end.writeUInt16LE(files.size, 10);
  end.writeUInt32LE(directory.length, 12); end.writeUInt32LE(offset, 16);
  return Buffer.concat([...local, directory, end]);
}

export function readZip(buffer) {
  const ensure = (condition, message = 'Damaged or unsupported ZIP32 archive') => { if (!condition) throw new Error(message); };
  ensure(buffer.length >= 22, 'Not a ZIP archive');
  let end = buffer.length - 22;
  const floor = Math.max(0, end - 65535);
  while (end >= floor && (buffer.readUInt32LE(end) !== 0x06054b50 || end + 22 + buffer.readUInt16LE(end + 20) !== buffer.length)) end--;
  ensure(end >= floor, 'Not a ZIP archive: no end-of-central-directory record');
  const count = buffer.readUInt16LE(end + 10), start = buffer.readUInt32LE(end + 16);
  ensure(!buffer.readUInt16LE(end + 4) && !buffer.readUInt16LE(end + 6) && buffer.readUInt16LE(end + 8) === count);
  ensure(count !== 65535 && start !== 0xffffffff && start + buffer.readUInt32LE(end + 12) === end);
  const entries = new Map(), names = new Set();
  let at = start, total = 0;
  for (let i = 0; i < count; i++) {
    ensure(at + 46 <= end && buffer.readUInt32LE(at) === 0x02014b50);
    const flags = buffer.readUInt16LE(at + 8), method = buffer.readUInt16LE(at + 10), crc = buffer.readUInt32LE(at + 16);
    const packed = buffer.readUInt32LE(at + 20), size = buffer.readUInt32LE(at + 24), length = buffer.readUInt16LE(at + 28), local = buffer.readUInt32LE(at + 42);
    const next = at + 46 + length + buffer.readUInt16LE(at + 30) + buffer.readUInt16LE(at + 32);
    ensure(next <= end && flags === 0 && (method === 0 || method === 8) && buffer.readUInt16LE(at + 34) === 0);
    const name = buffer.toString('utf8', at + 46, at + 46 + length);
    checkName(name);
    ensure(!names.has(name.toLowerCase()), `Duplicate ZIP entry: ${name}`);
    names.add(name.toLowerCase());
    ensure(local + 30 <= start && buffer.readUInt32LE(local) === 0x04034b50);
    ensure(buffer.readUInt16LE(local + 6) === flags && buffer.readUInt16LE(local + 8) === method && buffer.readUInt32LE(local + 14) === crc);
    ensure(buffer.readUInt32LE(local + 18) === packed && buffer.readUInt32LE(local + 22) === size && buffer.readUInt16LE(local + 26) === length);
    const dataAt = local + 30 + length + buffer.readUInt16LE(local + 28);
    ensure(dataAt + packed <= start && buffer.toString('utf8', local + 30, local + 30 + length) === name);
    total += size;
    ensure(total <= 512 * 1024 * 1024, 'ZIP exceeds the 512 MiB extraction budget');
    entries.set(name, { size, read() {
      const raw = buffer.subarray(dataAt, dataAt + packed);
      const data = method === 0 ? Buffer.from(raw) : inflateRawSync(raw, { maxOutputLength: Math.max(1, size) });
      ensure(data.length === size && crc32(data) === crc, `ZIP entry ${name} is damaged`);
      return data;
    } });
    at = next;
  }
  ensure(at === end);
  return entries;
}
