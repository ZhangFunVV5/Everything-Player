export interface Mp4Segment {
  byteOffset: number;
  start: number;
  end: number;
}

export interface Mp4SegmentIndex {
  initialization: Uint8Array;
  segments: Mp4Segment[];
}

function boxType(bytes: Uint8Array, offset: number): string {
  return String.fromCharCode(...bytes.subarray(offset + 4, offset + 8));
}

// Bilibili's fragmented MP4 provides a top-level sidx before the first moof.
// Reject indirect or incomplete indexes; never guess an MP4 byte offset.
export function parseMp4SegmentIndex(bytes: Uint8Array): Mp4SegmentIndex | null {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let offset = 0;
  let sidxOffset = -1;
  let sidxSize = 0;
  let firstMoof = -1;
  while (offset + 8 <= bytes.length) {
    const size = view.getUint32(offset);
    if (size < 8 || offset + size > bytes.length) return null;
    const type = boxType(bytes, offset);
    if (type === 'sidx') { sidxOffset = offset; sidxSize = size; }
    if (type === 'moof') { firstMoof = offset; break; }
    offset += size;
  }
  if (sidxOffset < 0 || firstMoof < 0) return null;
  const version = view.getUint8(sidxOffset + 8);
  const timescale = view.getUint32(sidxOffset + 16);
  if (!timescale || (version !== 0 && version !== 1)) return null;
  let cursor = sidxOffset + 20;
  const earliest = version === 0 ? view.getUint32(cursor) : Number(view.getBigUint64(cursor));
  cursor += version === 0 ? 4 : 8;
  const firstOffset = version === 0 ? view.getUint32(cursor) : Number(view.getBigUint64(cursor));
  cursor += version === 0 ? 4 : 8;
  cursor += 2;
  const count = view.getUint16(cursor);
  cursor += 2;
  if (!count || cursor + count * 12 > sidxOffset + sidxSize) return null;
  let byteOffset = sidxOffset + sidxSize + firstOffset;
  let time = earliest / timescale;
  const segments: Mp4Segment[] = [];
  for (let index = 0; index < count; index += 1) {
    const reference = view.getUint32(cursor);
    const duration = view.getUint32(cursor + 4) / timescale;
    if (reference >>> 31 || !duration) return null;
    const size = reference & 0x7fffffff;
    segments.push({ byteOffset, start: time, end: time + duration });
    byteOffset += size;
    time += duration;
    cursor += 12;
  }
  if (segments[0]?.byteOffset !== firstMoof) return null;
  return { initialization: bytes.slice(0, firstMoof), segments };
}

export function segmentAtTime(index: Mp4SegmentIndex, seconds: number): Mp4Segment | null {
  const segment = index.segments.find((entry) => seconds >= entry.start && seconds < entry.end);
  if (segment) return segment;
  const last = index.segments.at(-1);
  return last && Math.abs(seconds - last.end) < 0.05 ? last : null;
}
