export type LegacyMigrationFileStatus =
  | 'importable'
  | 'changed'
  | 'migrated'
  | 'partial'
  | 'failed'
  | 'invalid'
  | 'unsupported'
  | 'readError'
  | 'tooLarge'
  | 'empty';

export interface LegacyMigrationFile {
  name: string;
  size: number;
  status: LegacyMigrationFileStatus;
  folderCount: number;
  targetFolderIds: string[];
  reason?: string;
}

export interface LegacyMigrationPreview {
  sessionId: string;
  files: LegacyMigrationFile[];
  canCommit: boolean;
}

export interface LegacyMigrationResult {
  files: LegacyMigrationFile[];
  importedFolders: number;
  recordError?: string;
}
