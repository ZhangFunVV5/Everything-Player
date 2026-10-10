import type { MediaService, PlaybackSession } from '../platform/media-service.js';
import type { ServiceResult } from '../types/errors.js';
import type { BvInput, ResolvedVideo } from '../types/media.js';

export class PlayerApplicationService {
  private playback: PlaybackSession | null = null;
  private playbackVideo: ResolvedVideo | null = null;
  private rollback: { video: ResolvedVideo; time: number; wasPlaying: boolean } | null = null;
  private playbackVersion = 0;
  private pendingPlayback: Promise<void> = Promise.resolve();

  constructor(private readonly mediaService: MediaService) {}

  resolveForPlayback(input: BvInput): Promise<ServiceResult<ResolvedVideo>> {
    return this.mediaService.resolve(input);
  }

  invalidatePendingAudioLoad(): void {
    this.playbackVersion += 1;
  }

  commitPlayback(): void {
    this.rollback = null;
  }

  loadAudio(audio: HTMLAudioElement, video: ResolvedVideo, onError: (message: string) => void, initialTime?: number): Promise<void> {
    if (!this.rollback && this.playback && this.playbackVideo) {
      this.rollback = {
        video: this.playbackVideo,
        time: audio.currentTime,
        wasPlaying: !audio.paused,
      };
    }
    audio.pause();
    const version = ++this.playbackVersion;
    const work = this.pendingPlayback.catch(() => {}).then(async () => {
      if (version !== this.playbackVersion) return;
      await this.discardPlayback();
      if (version !== this.playbackVersion) return;
      const playback = await this.mediaService.createPlayback(audio, video, onError, initialTime);
      if (version !== this.playbackVersion) {
        await playback.dispose();
      } else {
        this.playback = playback;
        this.playbackVideo = video;
      }
    });
    this.pendingPlayback = work;
    return work;
  }

  async restoreAfterFailedResolve(
    audio: HTMLAudioElement,
    onError: (message: string) => void,
    isCurrent: () => boolean,
  ): Promise<void> {
    if (!isCurrent()) return;
    const rollback = this.rollback;
    if (!rollback) {
      await this.pendingPlayback.catch(() => {});
      return;
    }
    await this.loadAudio(audio, rollback.video, onError, rollback.time);
    if (!isCurrent()) return;
    audio.currentTime = rollback.time;
    this.commitPlayback();
    if (rollback.wasPlaying) await audio.play().catch(() => {});
  }

  async seekAudio(seconds: number): Promise<void> {
    await this.playback?.seek(seconds);
  }

  async resumeAudio(): Promise<void> {
    await this.playback?.resume();
  }

  disposeAudio(): Promise<void> {
    this.playbackVersion += 1;
    this.rollback = null;
    const work = this.pendingPlayback.catch(() => {}).then(() => this.discardPlayback());
    this.pendingPlayback = work;
    return work;
  }

  private async discardPlayback(): Promise<void> {
    const previous = this.playback;
    this.playback = null;
    this.playbackVideo = null;
    await previous?.dispose();
  }
}
