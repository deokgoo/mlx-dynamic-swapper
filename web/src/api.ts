function jsonErrorMessage(body: string): string | undefined {
  try {
    const data: unknown = JSON.parse(body);
    if (!data || typeof data !== 'object') return;

    const error = 'error' in data ? data.error : undefined;
    if (typeof error === 'string' && error.trim()) return error.trim();
    if (error && typeof error === 'object' && 'message' in error
      && typeof error.message === 'string' && error.message.trim()) {
      return error.message.trim();
    }
    if ('message' in data && typeof data.message === 'string' && data.message.trim()) {
      return data.message.trim();
    }
  } catch {
    // Malformed JSON errors still need a useful HTTP-status fallback.
  }
}

/** Reject failed actions without consuming successful response bodies. */
export async function throwForApiError(response: Response): Promise<void> {
  if (response.ok) return;

  const body = (await response.text()).trim();
  const mediaType = response.headers.get('content-type')?.split(';')[0].trim().toLowerCase() ?? '';
  const isJson = mediaType.endsWith('/json') || mediaType.endsWith('+json');
  const message = isJson ? jsonErrorMessage(body) : body;

  throw new Error(message || `HTTP ${response.status}`);
}
