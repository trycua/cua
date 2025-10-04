import { DockerComputer, OSType } from "@trycua/computer";
import { writeFileSync } from "node:fs";

async function main() {
  console.log("Initializing Docker Computer...");

  // Create a Docker computer instance
  const computer = new DockerComputer({
    name: "my-cua-container",
    osType: OSType.LINUX,
    image: "trycua/cua-ubuntu:latest",
    memory: "4GB",
    cpu: 2,
    port: 8000,
    vncPort: 6901,
    ephemeral: false,
  });

  try {
    // Start the Docker container and connect to it
    console.log("Starting Docker container...");
    await computer.run();
    console.log("Docker container is ready!");

    // Take a screenshot to verify it's working
    console.log("Taking screenshot...");
    const screenshot = await computer.interface.screenshot();
    writeFileSync("screenshot_docker.png", screenshot);
    console.log("Screenshot saved as screenshot_docker.png");

    // Example: Execute a command in the container
    console.log("Executing command...");
    const result = await computer.interface.shell("echo 'Hello from Docker container!'");
    console.log("Command output:", result);

    // Keep the container running for demonstration
    console.log("\nContainer is running! Press Ctrl+C to stop.");
    console.log(`VNC: http://localhost:${6901}`);
    console.log(`API: http://localhost:${8000}`);

    // Wait for user to stop
    await new Promise(() => {});
  } catch (error) {
    console.error("Error:", error);
  } finally {
    // Cleanup
    console.log("\nStopping Docker container...");
    await computer.stop();
    console.log("Docker container stopped");
  }
}

main().catch(console.error);
