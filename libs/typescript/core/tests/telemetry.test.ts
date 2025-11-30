import { beforeEach, describe, expect, it } from 'vitest';
import { Telemetry } from '../src/';

describe('Telemetry', () => {
  let telemetry: Telemetry;
  beforeEach(() => {
    process.env.CUA_TELEMETRY_ENABLED = '';
    telemetry = new Telemetry();
  });
  describe('telemetry.enabled', () => {
    it('should return false when CUA_TELEMETRY_ENABLED is false', () => {
      process.env.CUA_TELEMETRY_ENABLED = 'false';
      telemetry = new Telemetry();
      expect(telemetry.enabled).toBe(false);
    });

    it('should return false when CUA_TELEMETRY_ENABLED is 0', () => {
      process.env.CUA_TELEMETRY_ENABLED = '0';
      telemetry = new Telemetry();
      expect(telemetry.enabled).toBe(false);
    });

    it('should return true when CUA_TELEMETRY_ENABLED is not set (default)', () => {
      delete process.env.CUA_TELEMETRY_ENABLED;
      telemetry = new Telemetry();
      expect(telemetry.enabled).toBe(true);
    });

    it('should return true when CUA_TELEMETRY_ENABLED is true', () => {
      process.env.CUA_TELEMETRY_ENABLED = 'true';
      telemetry = new Telemetry();
      expect(telemetry.enabled).toBe(true);
    });

    it('should return true when CUA_TELEMETRY_ENABLED is 1', () => {
      process.env.CUA_TELEMETRY_ENABLED = '1';
      telemetry = new Telemetry();
      expect(telemetry.enabled).toBe(true);
    });
  });
});
