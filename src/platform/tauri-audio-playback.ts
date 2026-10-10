import { invokeMedia } from './p7-proof-invoke.js';
import { parseMp4SegmentIndex, segmentAtTime, type Mp4SegmentIndex } from './mp4-segment-index.js';
import type { PlaybackSession } from './media-service.js';

interface AudioHead {
  requestId: string;
  status: number;
  headers: Record<string, string>;
}

interface Transfer {
  requestId: string;
  range: string | null;
}

const MAX_INIT_BYTES = 64 * 1024;
const BUFFER_AHEAD_SECONDS = 20;
const BUFFER_BEHIND_SECONDS = 30;

function ipcBytes(value: unknown): Uint8Array {
  if (value instanceof Uint8Array) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  throw new Error('音频读取结果无效');
}

function waitForUpdate(buffer: SourceBuffer): Promise<void> {
  if (!buffer.updating) return Promise.resolve();
  return new Promise((resolve, reject) => {
    const cleanup = (): void => {
      buffer.removeEventListener('updateend', finish);
      buffer.removeEventListener('error', fail);
    };
    const finish = (): void => { cleanup(); resolve(); };
    const fail = (): void => { cleanup(); reject(new Error('音频片段无法解码')); };
    buffer.addEventListener('updateend', finish, { once: true });
    buffer.addEventListener('error', fail, { once: true });
  });
}

export class TauriAudioPlayback implements PlaybackSession {
  private mediaSource: MediaSource | null = null;
  private sourceBuffer: SourceBuffer | null = null;
  private objectUrl: string | null = null;
  private transfer: Transfer | null = null;
  private cancelPromise: Promise<void> = Promise.resolve();
  private generation = 0;
  private index: Mp4SegmentIndex | null = null;
  private initBytes = new Uint8Array(MAX_INIT_BYTES);
  private initLength = 0;
  private etag: string | null = null;
  private disposed = false;

  constructor(
    private readonly audio: HTMLAudioElement,
    private readonly token: string,
    private readonly onError: (message: string) => void,
  ) {}

  private readonly onPause = (): void => {
    this.generation += 1;
    this.cancelPromise = this.cancelActive();
  };

  async start(initialTime = 0): Promise<void> {
    const run = ++this.generation;
    const head = await this.open(null, run);
    if (!head) return;
    if (initialTime > 0) {
      await this.readIndex(run);
      if (run !== this.generation || this.disposed) return;
      await this.cancelActive();
    }
    const contentType = head.headers['content-type'] ?? 'audio/mp4';
    const mime = contentType.includes('mp4') ? 'audio/mp4; codecs="mp4a.40.2"' : contentType;
    if (!MediaSource.isTypeSupported(mime)) throw new Error('此音频格式不受播放器支持');
    const source = new MediaSource();
    this.mediaSource = source;
    this.objectUrl = URL.createObjectURL(source);
    this.audio.src = this.objectUrl;
    this.audio.load();
    await new Promise<void>((resolve, reject) => {
      source.addEventListener('sourceopen', () => resolve(), { once: true });
      source.addEventListener('error', () => reject(new Error('音频流初始化失败')), { once: true });
    });
    if (run !== this.generation || this.disposed) return;
    this.sourceBuffer = source.addSourceBuffer(mime);
    this.audio.addEventListener('pause', this.onPause);
    if (initialTime > 0) {
      const segment = this.index ? segmentAtTime(this.index, initialTime) : null;
      if (!segment || !this.index) throw new Error('此音频缺少可用的片段索引');
      await this.append(this.index.initialization, run);
      if (run !== this.generation || this.disposed) return;
      this.audio.currentTime = initialTime;
      const ranged = await this.open(`bytes=${segment.byteOffset}-`, run);
      if (!ranged) return;
    }
    void this.pump(run).catch((error: unknown) => this.report(error, run));
  }

  private async readIndex(run: number): Promise<void> {
    while (!this.index && this.initLength < MAX_INIT_BYTES && run === this.generation && !this.disposed) {
      const transfer = this.transfer;
      if (!transfer) return;
      const bytes = ipcBytes(await invokeMedia<unknown>('media_audio_read', { requestId: transfer.requestId }));
      if (run !== this.generation || this.transfer !== transfer || this.disposed) return;
      if (!bytes.length) break;
      const length = Math.min(bytes.length, MAX_INIT_BYTES - this.initLength);
      this.initBytes.set(bytes.subarray(0, length), this.initLength);
      this.initLength += length;
      this.index = parseMp4SegmentIndex(this.initBytes.subarray(0, this.initLength));
    }
    if (!this.index && run === this.generation && !this.disposed) throw new Error('此音频缺少可用的片段索引');
  }

  private async open(range: string | null, run: number): Promise<AudioHead | null> {
    const requestId = crypto.randomUUID();
    const head = await invokeMedia<AudioHead>('media_audio_open', {
      requestId,
      token: this.token,
      method: 'GET',
      range,
      ifRange: range ? this.etag : null,
    });
    if (run !== this.generation || this.disposed) {
      await invokeMedia('media_audio_cancel', { requestId });
      return null;
    }
    this.transfer = { requestId, range };
    this.etag = head.headers.etag ?? this.etag;
    if (range && head.status !== 206) {
      await this.cancelActive();
      throw new Error('音频片段请求失败');
    }
    return head;
  }

  private async cancelActive(): Promise<void> {
    const transfer = this.transfer;
    this.transfer = null;
    if (transfer) await invokeMedia('media_audio_cancel', { requestId: transfer.requestId });
  }

  private bufferedAhead(): number {
    const ranges = this.audio.buffered;
    for (let index = 0; index < ranges.length; index += 1) {
      if (this.audio.currentTime >= ranges.start(index) && this.audio.currentTime <= ranges.end(index)) {
        return ranges.end(index) - this.audio.currentTime;
      }
    }
    return 0;
  }

  private async append(bytes: Uint8Array, run: number): Promise<void> {
    const buffer = this.sourceBuffer;
    if (!buffer) return;
    await waitForUpdate(buffer);
    if (run !== this.generation || this.disposed) return;
    buffer.appendBuffer(bytes as Uint8Array<ArrayBuffer>);
    await waitForUpdate(buffer);
    if (run !== this.generation || this.disposed) return;
    const first = buffer.buffered.length ? buffer.buffered.start(0) : 0;
    const before = Math.max(0, this.audio.currentTime - BUFFER_BEHIND_SECONDS);
    if (before > first + 1) {
      buffer.remove(first, before);
      await waitForUpdate(buffer);
    }
  }

  private async pump(run: number): Promise<void> {
    while (run === this.generation && this.transfer && !this.disposed) {
      if (this.bufferedAhead() >= BUFFER_AHEAD_SECONDS) {
        await new Promise((resolve) => setTimeout(resolve, 250));
        continue;
      }
      const transfer = this.transfer;
      const bytes = ipcBytes(await invokeMedia<unknown>('media_audio_read', { requestId: transfer.requestId }));
      if (run !== this.generation || this.transfer !== transfer || this.disposed) return;
      if (!bytes.length) {
        this.transfer = null;
        if (this.mediaSource?.readyState === 'open') {
          await waitForUpdate(this.sourceBuffer!);
          this.mediaSource.endOfStream();
        }
        return;
      }
      if (!transfer.range && !this.index && this.initLength < MAX_INIT_BYTES) {
        const length = Math.min(bytes.length, MAX_INIT_BYTES - this.initLength);
        this.initBytes.set(bytes.subarray(0, length), this.initLength);
        this.initLength += length;
        this.index = parseMp4SegmentIndex(this.initBytes.subarray(0, this.initLength));
      }
      await this.append(bytes, run);
    }
  }

  async seek(seconds: number): Promise<void> {
    const index = this.index;
    const segment = index ? segmentAtTime(index, seconds) : null;
    if (!index || !segment) throw new Error('此音频缺少可用的片段索引');
    const run = ++this.generation;
    await this.cancelPromise;
    await this.cancelActive();
    const buffer = this.sourceBuffer;
    if (!buffer || this.disposed) return;
    await waitForUpdate(buffer);
    if (buffer.buffered.length) {
      buffer.remove(buffer.buffered.start(0), buffer.buffered.end(buffer.buffered.length - 1));
      await waitForUpdate(buffer);
    }
    buffer.abort();
    buffer.timestampOffset = 0;
    await this.append(index.initialization, run);
    const head = await this.open(`bytes=${segment.byteOffset}-`, run);
    if (head) void this.pump(run).catch((error: unknown) => this.report(error, run));
  }

  async resume(): Promise<void> {
    await this.cancelPromise;
    if (!this.transfer && this.mediaSource?.readyState === 'open' && !this.disposed) {
      if (this.index) {
        await this.seek(this.audio.currentTime);
      } else {
        const run = ++this.generation;
        const buffer = this.sourceBuffer;
        if (!buffer) return;
        await waitForUpdate(buffer);
        if (buffer.buffered.length) {
          buffer.remove(buffer.buffered.start(0), buffer.buffered.end(buffer.buffered.length - 1));
          await waitForUpdate(buffer);
        }
        buffer.abort();
        this.initLength = 0;
        const head = await this.open(null, run);
        if (head) void this.pump(run).catch((error: unknown) => this.report(error, run));
      }
    }
  }

  async dispose(): Promise<void> {
    this.disposed = true;
    this.generation += 1;
    this.audio.removeEventListener('pause', this.onPause);
    await this.cancelPromise;
    await this.cancelActive();
    this.audio.removeAttribute('src');
    this.audio.load();
    if (this.objectUrl) URL.revokeObjectURL(this.objectUrl);
    this.sourceBuffer = null;
    this.mediaSource = null;
  }

  private report(error: unknown, run: number): void {
    if (run !== this.generation || this.disposed) return;
    this.cancelPromise = this.cancelActive();
    const message = typeof error === 'string' ? error : error instanceof Error ? error.message : '音频播放失败';
    this.onError(message);
  }
}
