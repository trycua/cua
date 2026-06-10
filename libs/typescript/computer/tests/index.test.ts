import { describe, expect, it } from 'vitest';
import * as ComputerExports from '../src/index.ts';

describe('package exports', () => {
  it('exports host computer interfaces from the package root', () => {
    expect(ComputerExports.MacOSComputerInterface).toBeDefined();
    expect(ComputerExports.LinuxComputerInterface).toBeDefined();
    expect(ComputerExports.WindowsComputerInterface).toBeDefined();
  });
});
