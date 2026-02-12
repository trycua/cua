declare module '@lydell/node-pty' {
  export interface IPtyForkOptions {
    name?: string;
    cols?: number;
    rows?: number;
    cwd?: string;
    env?: { [key: string]: string | undefined };
    encoding?: string | null;
    handleFlowControl?: boolean;
    flowControlPause?: string;
    flowControlResume?: string;
  }

  export interface IPty {
    readonly pid: number;
    readonly cols: number;
    readonly rows: number;
    readonly process: string;
    readonly handleFlowControl: boolean;
    onData: (callback: (data: string) => void) => void;
    onExit: (callback: (exitCode: { exitCode: number; signal?: number }) => void) => void;
    on(event: 'data', callback: (data: string) => void): void;
    on(event: 'exit', callback: (exitCode: number, signal?: number) => void): void;
    resize(cols: number, rows: number): void;
    clear(): void;
    write(data: string): void;
    kill(signal?: string): void;
  }

  export function spawn(file: string, args: string[] | string, options: IPtyForkOptions): IPty;
}
