import { describe, it, expect } from 'vitest';
import { Transport } from '../../src/transport.js';

describe('Transport', () => {
  it('buildUrl strips trailing slash from baseUrl', () => {
    const t = new Transport('http://localhost:8402/');
    // Access via the public sendChat path building — we test indirectly via fetchModels URL
    expect(t['buildUrl']('/v1/models')).toBe('http://localhost:8402/v1/models');
  });

  it('buildUrl works without trailing slash', () => {
    const t = new Transport('http://localhost:8402');
    expect(t['buildUrl']('/v1/chat/completions')).toBe(
      'http://localhost:8402/v1/chat/completions',
    );
  });

  it('buildHeaders includes Content-Type', () => {
    const t = new Transport('http://localhost:8402');
    const headers = t['buildHeaders']();
    expect(headers['Content-Type']).toBe('application/json');
  });

  it('buildHeaders includes Payment-Signature when provided', () => {
    const t = new Transport('http://localhost:8402');
    const headers = t['buildHeaders']('sig123');
    expect(headers['Payment-Signature']).toBe('sig123');
  });

  it('buildHeaders omits Payment-Signature when not provided', () => {
    const t = new Transport('http://localhost:8402');
    const headers = t['buildHeaders']();
    expect(headers).not.toHaveProperty('Payment-Signature');
  });

  it('buildHeaders merges extra headers', () => {
    const t = new Transport('http://localhost:8402');
    const headers = t['buildHeaders'](undefined, { 'X-Custom': 'value' });
    expect(headers['X-Custom']).toBe('value');
    expect(headers['Content-Type']).toBe('application/json');
  });

  it('constructor sets default timeout', () => {
    const t = new Transport('http://localhost:8402');
    expect(t['timeout']).toBe(180);
  });

  it('constructor accepts custom timeout', () => {
    const t = new Transport('http://localhost:8402', 60);
    expect(t['timeout']).toBe(60);
  });
});
