import { describe, expect, it } from 'vitest';
import { DockerComputer } from '../../src';
import { OSType } from '../../src/types';

describe('Computer Docker', () => {
  it('Should create docker computer instance', () => {
    const docker = new DockerComputer({
      name: 'test-container',
      osType: OSType.LINUX,
      image: 'trycua/cua-ubuntu:latest',
      port: 8000,
      vncPort: 6901,
    });
    expect(docker).toBeInstanceOf(DockerComputer);
  });

  it('Should create docker computer instance with default values', () => {
    const docker = new DockerComputer({
      name: 'test-container',
      osType: OSType.LINUX,
    });
    expect(docker).toBeInstanceOf(DockerComputer);
  });

  it('Should have correct name and OS type', () => {
    const docker = new DockerComputer({
      name: 'my-test-container',
      osType: OSType.LINUX,
    });
    expect(docker.getName()).toBe('my-test-container');
    expect(docker.getOSType()).toBe(OSType.LINUX);
  });
});
