import type { ServiceResult } from '../types/errors.js';
import type {
  LegacyMigrationPreview,
  LegacyMigrationResult,
} from '../types/legacy-migration.js';

export interface LegacyMigrationService {
  select(): Promise<ServiceResult<LegacyMigrationPreview | null>>;
  cancel(sessionId: string): Promise<ServiceResult<void>>;
  commit(sessionId: string): Promise<ServiceResult<LegacyMigrationResult>>;
}
