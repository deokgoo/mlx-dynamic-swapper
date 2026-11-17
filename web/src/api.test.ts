import { describe, expect, it } from 'vitest';
import { throwForApiError } from './api';

describe('throwForApiError', () => {
  it('leaves a successful response available for the caller', async () => {
    const response = Response.json({ completed: true });

    await throwForApiError(response);

    expect(response.bodyUsed).toBe(false);
    expect(await response.json()).toEqual({ completed: true });
  });

  it.each([
    { body: '{"error":"backend failure"}', type: 'application/json', message: 'backend failure' },
    { body: '{"message":"Try again later"}', type: 'application/json; charset=utf-8', message: 'Try again later' },
    { body: '{"error":{"message":"Model unavailable"}}', type: 'application/json', message: 'Model unavailable' },
    { body: '{"message":"Workflow failed"}', type: 'application/problem+json', message: 'Workflow failed' },
    { body: '  Template not found\n', type: 'text/plain', message: 'Template not found' },
    { body: '', type: 'text/plain', message: 'HTTP 500' },
    { body: '  \n', type: 'text/plain', message: 'HTTP 500' },
    { body: '', type: 'application/json', message: 'HTTP 500' },
    { body: '{invalid json', type: 'application/json', message: 'HTTP 500' },
    { body: '{"error":42}', type: 'application/json', message: 'HTTP 500' },
    { body: '{"error":"   "}', type: 'application/json', message: 'HTTP 500' },
    { body: 'null', type: 'application/json', message: 'HTTP 500' },
  ])('reports $message for $type body $body', async ({ body, type, message }) => {
    const response = new Response(body, {
      status: 500,
      headers: { 'Content-Type': type },
    });

    await expect(throwForApiError(response)).rejects.toThrow(message);
  });
});
