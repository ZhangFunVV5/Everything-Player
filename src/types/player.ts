import type { Bvid } from './media.js';

export type PlayMode = 'sequence' | 'shuffle' | 'repeat';

export type PlayerPage = 'player' | 'profile';

export type PlayerSourceType = 'single' | 'collection' | 'favorite';

export type QueueDirection = 'previous' | 'next';

export interface QueueItem {
  bvid: Bvid;
  title: string;
  duration: number | null;
  coverUrl: string;
  sectionTitle: string;
}

export interface QueueBuildResult {
  title: string;
  items: QueueItem[];
  currentIndex: number;
}

export interface PlayerState {
  page: PlayerPage;
  sourceType: PlayerSourceType;
  sourceId: string | number | null;
  mode: PlayMode;
  queueTitle: string;
  queue: QueueItem[];
  currentIndex: number;
  randomHistory: number[];
  randomCursor: number;
}
