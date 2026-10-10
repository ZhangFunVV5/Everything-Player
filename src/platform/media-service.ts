import type { ServiceResult } from '../types/errors.js';
import type { BvInput, ResolvedVideo } from '../types/media.js';

export interface PlaybackSession {
  seek(seconds: number): Promise<void>;
  resume(): Promise<void>;
  dispose(): Promise<void>;
}

export interface MediaService {
  resolve(input: BvInput): Promise<ServiceResult<ResolvedVideo>>;
  createPlayback(audio: HTMLAudioElement, video: ResolvedVideo, onError: (message: string) => void, initialTime?: number): Promise<PlaybackSession>;
}
