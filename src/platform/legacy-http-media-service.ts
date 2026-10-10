import type { MediaService, PlaybackSession } from './media-service.js';
import type { ServiceErrorResponse, ServiceResult } from '../types/errors.js';
import type { BvInput, ResolvedVideo } from '../types/media.js';

function isServiceErrorResponse(value: unknown): value is ServiceErrorResponse {
  return typeof value === 'object'
    && value !== null
    && 'error' in value
    && typeof value.error === 'string';
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback;
}

export class LegacyHttpMediaService implements MediaService {
  async createPlayback(audio: HTMLAudioElement, video: ResolvedVideo): Promise<PlaybackSession> {
    audio.src = video.audioUrl;
    audio.load();
    return {
      seek: async () => {},
      resume: async () => {},
      dispose: async () => {},
    };
  }

  async resolve(input: BvInput): Promise<ServiceResult<ResolvedVideo>> {
    try {
      const response = await fetch('/api/resolve', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ input }),
      });
      const result: unknown = await response.json();
      if (!response.ok) {
        return {
          ok: false,
          error: {
            message: isServiceErrorResponse(result) ? result.error : '解析失败',
          },
        };
      }
      return { ok: true, value: result as ResolvedVideo };
    } catch (error: unknown) {
      return {
        ok: false,
        error: { message: errorMessage(error, '解析失败') },
      };
    }
  }
}
