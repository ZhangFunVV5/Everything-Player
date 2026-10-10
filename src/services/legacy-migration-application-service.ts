import type { LegacyMigrationService } from '../platform/legacy-migration-service.js';
import type { ServiceResult } from '../types/errors.js';
import type {
  LegacyMigrationPreview,
  LegacyMigrationResult,
} from '../types/legacy-migration.js';

export class LegacyMigrationApplicationService {
  constructor(private readonly migrationService: LegacyMigrationService) {}

  select(): Promise<ServiceResult<LegacyMigrationPreview | null>> {
    return this.migrationService.select();
  }

  cancel(sessionId: string): Promise<ServiceResult<void>> {
    return this.migrationService.cancel(sessionId);
  }

  commit(sessionId: string): Promise<ServiceResult<LegacyMigrationResult>> {
    return this.migrationService.commit(sessionId);
  }
}
