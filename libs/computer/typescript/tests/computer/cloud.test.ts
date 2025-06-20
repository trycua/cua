import { describe, expect, it } from "vitest";
import { OSType } from "../../src/types";
import { CloudComputer } from "../../src/computer/providers/cloud";

describe("Computer Cloud", () => {
  it("Should create computer instance", () => {
    const cloud = new CloudComputer({
      apiKey: "asdf",
      name: "s-linux-1234",
      osType: OSType.LINUX,
    });
    expect(cloud).toBeInstanceOf(CloudComputer);
  });
});
