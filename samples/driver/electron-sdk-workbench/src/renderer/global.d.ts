export {};

declare global {
  interface Window {
    cuaWorkbench: {
      getStatus(): Promise<unknown>;
      getPermissions(): Promise<unknown>;
      requestPermissions(): Promise<unknown>;
      openScreenRecordingSettings(): Promise<unknown>;
      startSession(request: unknown): Promise<unknown>;
      endSession(): Promise<unknown>;
      invokeTool(request: unknown): Promise<unknown>;
    };
  }
}
