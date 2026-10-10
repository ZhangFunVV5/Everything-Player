export interface ServiceErrorResponse {
  error: string;
}

export interface ServiceFailure {
  message: string;
}

export type ServiceResult<T> =
  | { ok: true; value: T }
  | { ok: false; error: ServiceFailure };
