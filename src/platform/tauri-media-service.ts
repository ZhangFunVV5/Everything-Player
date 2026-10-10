import { invokeMedia } from './p7-proof-invoke.js';
import type { MediaService, PlaybackSession } from './media-service.js';
import { TauriAudioPlayback } from './tauri-audio-playback.js';
import type { ServiceResult } from '../types/errors.js';
import type { BvInput, ResolvedVideo } from '../types/media.js';

const FALLBACK_ERROR = '解析失败';
const SENSITIVE_ERROR_PATTERN = /(?:cookie|authorization|bearer|credential|password|secret|token)\b/i;

function safeMessage(message: string): string {
  const trimmed = message.trim();
  return trimmed
    && !trimmed.includes('/')
    && !trimmed.includes('\\')
    && !SENSITIVE_ERROR_PATTERN.test(trimmed)
    ? trimmed
    : FALLBACK_ERROR;
}

function errorMessage(error: unknown): string {
  if (typeof error === 'string') return safeMessage(error);
  if (error instanceof Error) return safeMessage(error.message);
  if (typeof error === 'object' && error !== null && 'message' in error) {
    const message = error.message;
    if (typeof message === 'string') return safeMessage(message);
  }
  return FALLBACK_ERROR;
}

export class TauriMediaService implements MediaService {
  async createPlayback(audio: HTMLAudioElement, video: ResolvedVideo, onError: (message: string) => void, initialTime?: number): Promise<PlaybackSession> {
    const token = /^media-token:\/\/([0-9a-f-]{36})$/i.exec(video.audioUrl)?.[1];
    if (!token) throw new Error('媒体播放地址无效');
    const playback = new TauriAudioPlayback(audio, token, onError);
    try {
      await playback.start(initialTime);
    } catch (error) {
      await playback.dispose();
      throw error;
    }
    return playback;
  }

  resolve(input: BvInput): Promise<ServiceResult<ResolvedVideo>> {
    return invokeMedia<ResolvedVideo>('media_resolve', { input })
      .then((value) => ({ ok: true, value }) as ServiceResult<ResolvedVideo>)
      .catch((error: unknown) => ({
        ok: false,
        error: { message: errorMessage(error) },
      }));
  }
}
