import { Database } from "bun:sqlite";
import { getDbPath } from "./config";

function getDb(): Database {
  const db = new Database(getDbPath());
  db.exec("PRAGMA journal_mode = WAL;");
  db.exec("CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);");
  return db;
}

export function setApiKey(token: string) {
  const db = getDb();
  const stmt = db.query("INSERT INTO kv (k, v) VALUES ('api_key', ?) ON CONFLICT(k) DO UPDATE SET v=excluded.v");
  stmt.run(token);
}

export function getApiKey(): string | null {
  const db = getDb();
  const row = db.query("SELECT v FROM kv WHERE k='api_key'").get() as { v: string } | undefined;
  return row?.v ?? null;
}

export function clearApiKey() {
  const db = getDb();
  db.query("DELETE FROM kv WHERE k='api_key'").run();
}
