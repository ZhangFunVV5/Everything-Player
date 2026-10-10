import type { Bvid, BvInput } from './media.js';

export interface FavoriteItem {
  bvid: Bvid;
  title: string;
  owner: string;
  duration: number | null;
  coverUrl: string;
  addedAt: string;
}

export interface FavoriteFolder {
  schemaVersion: 1;
  id: string;
  name: string;
  description: string;
  createdAt: string;
  updatedAt: string;
  items: FavoriteItem[];
}

export interface FavoriteListResponse {
  folders: FavoriteFolder[];
  errors: string[];
}

export interface FavoriteCreateRequest {
  name: string;
  description?: string;
}

export interface FavoriteAddRequest {
  input: BvInput;
}

export interface FavoriteAddResponse {
  folder: FavoriteFolder;
  duplicate: boolean;
}

export interface FavoriteExportBundle {
  schemaVersion: 1;
  type: 'bili-sound-favorites';
  folders: FavoriteFolder[];
}

export type FavoriteImportData = FavoriteFolder | FavoriteExportBundle;

export interface FavoriteImportRequest {
  data: FavoriteImportData;
  preview?: boolean;
}

export interface FavoriteImportPreviewResponse {
  folders: FavoriteFolder[];
  preview: true;
}

export interface FavoriteImportCommitResponse {
  folders: FavoriteFolder[];
  error?: string;
}
