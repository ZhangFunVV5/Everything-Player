import { invoke } from '@tauri-apps/api/core';
import type { ServiceResult } from '../types/errors.js';
import type {
  LegacyMigrationPreview,
  LegacyMigrationResult,
} from '../types/legacy-migration.js';
import type { LegacyMigrationService } from './legacy-migration-service.js';

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'object' && error !== null && 'message' in error) {
    const message = error.message;
    if (typeof message === 'string') return message;
  }
  return '收藏夹迁移失败';
}

function invokeResult<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<ServiceResult<T>> {
  return invoke<T>(command, args)
    .then((value) => ({ ok: true, value }) as ServiceResult<T>)
    .catch((error: unknown) => ({
      ok: false,
      error: { message: errorMessage(error) },
    }));
}

export class TauriLegacyMigrationService implements LegacyMigrationService {
  select(): Promise<ServiceResult<LegacyMigrationPreview | null>> {
    return invokeResult<LegacyMigrationPreview | null>(
      'favorites_migration_select_legacy_directory',
    );
  }

  cancel(sessionId: string): Promise<ServiceResult<void>> {
    return invokeResult<void>('favorites_migration_cancel', { sessionId });
  }

  commit(sessionId: string): Promise<ServiceResult<LegacyMigrationResult>> {
    return invokeResult<LegacyMigrationResult>('favorites_migration_commit', { sessionId });
  }
}
