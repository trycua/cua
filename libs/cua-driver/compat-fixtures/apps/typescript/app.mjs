// Frozen cua-driver-rs-v0.12.6 application source.
import { CuaDriver } from "@trycua/cua-driver";

const driver = CuaDriver.connect(undefined);
const endpoint = driver.socketPath();
if (!endpoint.trim()) throw new Error("default endpoint must be selected");
console.log(endpoint);
driver.uniffiDestroy();
