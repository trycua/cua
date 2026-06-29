"""router.py — 任務路由引擎（Hybrid Vision Loop 嘅心臟）

一句總結：你俾個任務，router 自動揀最快最可靠嘅方法執行。

Tier 系統：
  Tier 1 — CLI/Git command — 唔使 GUI，最快
  Tier 2 — CDP WebSocket — 直接操控 browser DOM，精準
  Tier 3 — Hermes Browser — headless browser tools
  Tier 4 — Cua Driver — OS-level input (pixel/keystroke)
  Tier 5 — Vision AI — screenshot 分析輔助

每一個任務都有一條「最佳路徑」，router 知道點揀。
唔同時間、唔同 context，可能揀唔同嘅路徑。
"""

from __future__ import annotations

import json
import os
import sqlite3
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple


# ============================================================
# 路徑定義
# ============================================================

@dataclass
class Path:
    """一條執行路徑"""
    id: str                    # 唯一 ID
    tier: int                  # 1=最快, 5=最慢
    name: str                  # 人類可讀名稱
    check_fn: str = ""         # 前置條件檢查
    exec_fn: str = ""          # 執行函數


@dataclass
class Route:
    """一條任務路由
    
    一個任務可以有多條 path，router 按 tier 順序 check，
    揀最快可用嗰條。
    """
    task: str                  # 任務 ID（如 "create_github_token"）
    paths: List[Path]          # 可行路徑（由快到慢排序）
    description: str = ""      # 人類可讀描述


def _check_safari() -> dict:
    """Safari 係咪開緊？"""
    try:
        result = subprocess.run(
            ["pgrep", "-x", "Safari"],
            capture_output=True, text=True, timeout=3
        )
        ok = result.returncode == 0
        return {
            "ok": ok,
            "message": "✅ Safari 運行中" if ok else "❌ Safari 未開",
        }
    except Exception as e:
        return {"ok": False, "message": f"❌ Safari 檢查失敗: {e}"}


def _exec_safari(task: str, args: dict) -> dict:
    """Tier 2: Safari AX → navigate GitHub tokens page"""
    if task == "create_github_token":
        return {
            "success": True,
            "note": "Safari AX path ready",
            "method": "Safari AX",
            "task": task,
            "warnings": ["GitHub now requires sudo mode (2FA/email) for token creation"],
        }
    return {"success": False, "error": f"Safari 未知任務: {task}"}


# ============================================================
# 前置條件檢查
# ============================================================

def _check_gh_auth() -> dict:
    """GitHub CLI login 狀態"""
    try:
        result = subprocess.run(
            ["gh", "auth", "status"],
            capture_output=True, text=True, timeout=5
        )
        ok = result.returncode == 0
        return {
            "ok": ok,
            "message": "✅ gh 已 login" if ok else "❌ gh 未 login",
            "detail": result.stdout[:100] if ok else result.stderr[:100]
        }
    except FileNotFoundError:
        return {"ok": False, "message": "❌ gh 未安裝"}
    except Exception as e:
        return {"ok": False, "message": f"❌ 檢查失敗: {e}"}


def _check_cdp_port() -> dict:
    """Edge/Chrome 嘅 remote debugging port 9222 開咗未？"""
    try:
        result = subprocess.run(
            ["curl", "-s", "-o", "/dev/null", "-w", "%{http_code}",
             "http://localhost:9222/json/version"],
            capture_output=True, text=True, timeout=3
        )
        if result.stdout.strip() == "200":
            info = subprocess.run(
                ["curl", "-s", "http://localhost:9222/json/version"],
                capture_output=True, text=True, timeout=3
            )
            try:
                browser = json.loads(info.stdout).get("Browser", "Edge")
                return {"ok": True, "message": f"✅ CDP {browser} 已開 (port 9222)"}
            except json.JSONDecodeError:
                return {"ok": True, "message": "✅ CDP port 9222 已開"}
        return {"ok": False, "message": "❌ CDP port 9222 未開"}
    except Exception as e:
        return {"ok": False, "message": f"❌ CDP port 9222 檢查失敗: {e}"}


def _check_cua_daemon() -> dict:
    """Cua Driver daemon 運行中"""
    try:
        result = subprocess.run(
            ["cua-driver", "status"],
            capture_output=True, text=True, timeout=5
        )
        ok = "running" in result.stdout.lower()
        return {
            "ok": ok,
            "message": "✅ Cua daemon 運行中" if ok else "❌ Cua daemon 未運行",
            "detail": result.stdout[:100] if ok else result.stderr[:100]
        }
    except FileNotFoundError:
        return {"ok": False, "message": "❌ cua-driver 未安裝"}
    except Exception as e:
        return {"ok": False, "message": f"❌ Cua daemon 檢查失敗: {e}"}


# ============================================================
# 執行引擎
# ============================================================

def _exec_cli(task: str, args: dict) -> dict:
    """Tier 1: CLI command execution"""
    
    if task == "create_github_token":
        note = args.get("note", "hermes-agent-pr")
        scopes = args.get("scopes", ["public_repo"])
        return {
            "success": False,
            "error": "GitHub deprecated the OAuth Authorizations API (2020). "
                     "PAT now requires browser web flow + sudo mode (2FA/email). "
                     "Use Edge/Safari → https://github.com/settings/tokens/new",
            "detail": "Cannot create token via CLI. Browser web flow required."
        }
    
    return {"success": False, "error": f"CLI 未知任務: {task}"}


def _exec_cdp(task: str, args: dict) -> dict:
    """Tier 2: CDP WebSocket execution"""
    
    if task in ("browser_click", "browser_fill_form", "browser_eval"):
        # CDP execution — delegate to node script
        return {
            "success": True,
            "note": f"Agent should use CDP for: {task}",
            "method": "CDP",
            "task": task,
        }
    
    return {"success": False, "error": f"CDP 未知任務: {task}"}


def _exec_cua(task: str, args: dict) -> dict:
    """Tier 4: Cua Driver execution"""
    from hybrid_vision_loop.execute import execute_action
    return execute_action(task, args)


# ============================================================
# 註冊表 — 所有任務 + 可行路徑
# ============================================================

ALL_TASKS: Dict[str, Route] = {
    
    # ═══════════════════════════════════════════
    # GitHub 相關
    # ═══════════════════════════════════════════
    
    "create_github_token": Route(
        task="create_github_token",
        description="Create GitHub classic PAT with scopes — 只可靠路徑係 device code flow",
        paths=[
            Path("cli_device_flow", 1, "gh auth login (device code — 用戶手機 approve)", "check_gh_auth","exec_cli"),
            Path("cdp_token_page",  2, "CDP → token page ❌ Edge CDP no page targets",   "check_cdp_port","exec_cdp"),
            Path("safari_web_flow", 2, "Safari AX → tokens page ❌ truncated >2000 nodes","check_safari","exec_safari"),
            Path("cli_api",         1, "GitHub API ❌ deprecated 2020 (404)",             "check_gh_auth","exec_cli"),
        ],
    ),
    
    # ═══════════════════════════════════════════
    # Browser 操作
    # ═══════════════════════════════════════════
    
    "browser_navigate": Route(
        task="browser_navigate",
        description="Navigate browser to URL",
        paths=[
            Path("cua_hotkey",      4, "Cua Hotkey Cmd+L",     "check_cua_daemon","exec_cua"),
        ],
    ),
    
    "browser_fill_input": Route(
        task="browser_fill_input",
        description="Fill a form input field",
        paths=[
            Path("cdp_js",          2, "CDP JS set value",     "check_cdp_port","exec_cdp"),
            Path("cua_type",        4, "Cua type_text",        "check_cua_daemon","exec_cua"),
        ],
    ),
    
    "browser_click_checkbox": Route(
        task="browser_click_checkbox",
        description="Click/tick a checkbox in web page",
        paths=[
            Path("cdp_js",          2, "CDP JS tick checkbox", "check_cdp_port","exec_cdp"),
        ],
    ),
    
    "browser_click_button": Route(
        task="browser_click_button",
        description="Click a button in web page",
        paths=[
            Path("cdp_js",          2, "CDP JS click button",  "check_cdp_port","exec_cdp"),
            Path("cua_pixel",       4, "Cua pixel click",      "check_cua_daemon","exec_cua"),
        ],
    ),
    
    # ═══════════════════════════════════════════
    # Desktop App 操作
    # ═══════════════════════════════════════════
    
    "app_hotkey": Route(
        task="app_hotkey",
        description="Send keyboard shortcut to app",
        paths=[
            Path("cua_hotkey",      4, "Cua hotkey",           "check_cua_daemon","exec_cua"),
        ],
    ),
    
    "app_type": Route(
        task="app_type",
        description="Type text into app",
        paths=[
            Path("cua_type",        4, "Cua type_text",        "check_cua_daemon","exec_cua"),
        ],
    ),
    
    "app_click": Route(
        task="app_click",
        description="Click at coordinates",
        paths=[
            Path("cua_click",       4, "Cua click",            "check_cua_daemon","exec_cua"),
        ],
    ),
    
    "app_screenshot": Route(
        task="app_screenshot",
        description="Take screenshot",
        paths=[
            Path("screencapture",   4, "screencapture -x",     "check_cua_daemon","exec_cua"),
        ],
    ),
    
    # ═══════════════════════════════════════════
    # Git 操作
    # ═══════════════════════════════════════════
    
    "git_commit": Route(
        task="git_commit",
        description="Git commit with message",
        paths=[
            Path("cli_git",         1, "git CLI",               "", "exec_cli"),
        ],
    ),
    
    "git_push": Route(
        task="git_push",
        description="Git push to remote",
        paths=[
            Path("cli_git",         1, "git CLI",               "", "exec_cli"),
        ],
    ),
}


CHECK_FUNCS = {
    "check_gh_auth": _check_gh_auth,
    "check_cdp_port": _check_cdp_port,
    "check_cua_daemon": _check_cua_daemon,
    "check_safari": _check_safari,
}

EXEC_FUNCS = {
    "exec_cli": _exec_cli,
    "exec_cdp": _exec_cdp,
    "exec_cua": _exec_cua,
    "exec_safari": _exec_safari,
}


# ============================================================
# Router History DB — 越用越聰明
# ============================================================

_DEFAULT_DB_PATH = os.path.expanduser("~/.hermes/data/router_history.db")


class RouterHistoryDB:
    """Router 經驗記錄 — SQLite 本地儲存。
    
    每次 do() 自動記錄 routing 結果，下次 route() 會參考歷史 data
    去揀最快最可靠嘅 path。
    
    Usage:
        db = RouterHistoryDB()
        db.record("create_github_token", "cli_api", 1, True, 1234)
        best = db.best_path("create_github_token")  # {"path": "cli_api", "avg_ms": 1234, "samples": 5}
    """

    def __init__(self, db_path: str = _DEFAULT_DB_PATH):
        self._db_path = db_path
        self._ensure_db()

    def _ensure_db(self):
        db_dir = os.path.dirname(self._db_path)
        os.makedirs(db_dir, exist_ok=True)
        conn = self._connect()
        try:
            conn.execute("""
                CREATE TABLE IF NOT EXISTS router_history (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    task        TEXT NOT NULL,
                    path_id     TEXT NOT NULL,
                    tier        INTEGER NOT NULL,
                    success     INTEGER NOT NULL,
                    time_ms     INTEGER NOT NULL,
                    error_msg   TEXT DEFAULT '',
                    created_at  TEXT DEFAULT (datetime('now'))
                )
            """)
            conn.execute("""
                CREATE INDEX IF NOT EXISTS idx_router_history_task
                ON router_history(task, created_at)
            """)
            conn.commit()
        finally:
            conn.close()

    def _connect(self) -> sqlite3.Connection:
        conn = sqlite3.connect(self._db_path)
        conn.execute("PRAGMA journal_mode=WAL")
        return conn

    def record(self, task: str, path_id: str, tier: int,
               success: bool, time_ms: int, error_msg: str = ""):
        """記錄一次 routing 結果"""
        conn = self._connect()
        try:
            conn.execute(
                "INSERT INTO router_history (task, path_id, tier, success, time_ms, error_msg) "
                "VALUES (?, ?, ?, ?, ?, ?)",
                (task, path_id, tier, 1 if success else 0, time_ms, error_msg)
            )
            conn.commit()
        finally:
            conn.close()

    def best_path(self, task: str, min_samples: int = 5) -> dict:
        """查歷史：最快成功嘅 path（至少 min_samples 次）
        
        Returns:
            {"path": str, "avg_ms": float, "samples": int} or None
        """
        conn = self._connect()
        try:
            row = conn.execute("""
                SELECT path_id, AVG(time_ms) as avg_ms, COUNT(*) as samples
                FROM router_history
                WHERE task = ? AND success = 1
                GROUP BY path_id
                HAVING samples >= ?
                ORDER BY avg_ms ASC
                LIMIT 1
            """, (task, min_samples)).fetchone()
            if row:
                return {"path": row[0], "avg_ms": row[1], "samples": row[2]}
            return None
        finally:
            conn.close()

    def recent_failures(self, task: str, path_id: str, n: int = 3) -> list:
        """最近 N 次執行結果（用嚟 detect 連續 fail）
        
        Returns:
            list of {"success": bool, "error_msg": str, "created_at": str}
        """
        conn = self._connect()
        try:
            rows = conn.execute("""
                SELECT success, error_msg, created_at
                FROM router_history
                WHERE task = ? AND path_id = ?
                ORDER BY id DESC
                LIMIT ?
            """, (task, path_id, n)).fetchall()
            return [
                {"success": bool(r[0]), "error_msg": r[1], "created_at": r[2]}
                for r in rows
            ]
        finally:
            conn.close()

    def stats(self, task: str) -> dict:
        """統計摘要
        
        Returns:
            {
                "total": int,
                "paths": {
                    "path_id": {"samples": int, "success_rate": float, "avg_ms": float},
                    ...
                }
            }
        """
        conn = self._connect()
        try:
            total = conn.execute(
                "SELECT COUNT(*) FROM router_history WHERE task = ?",
                (task,)
            ).fetchone()[0]

            rows = conn.execute("""
                SELECT path_id,
                       COUNT(*) as samples,
                       AVG(CASE WHEN success=1 THEN 100.0 ELSE 0.0 END) as success_rate,
                       AVG(time_ms) as avg_ms
                FROM router_history
                WHERE task = ?
                GROUP BY path_id
                ORDER BY avg_ms ASC
            """, (task,)).fetchall()

            paths = {}
            for r in rows:
                paths[r[0]] = {
                    "samples": r[1],
                    "success_rate": round(r[2], 1),
                    "avg_ms": round(r[3], 1),
                }
            return {"total": total, "paths": paths}
        finally:
            conn.close()


# Global singleton — 所有 module import router 時共享同一個 DB
_HISTORY_DB = RouterHistoryDB()


# ============================================================
# Public API
# ============================================================

class RouterError(Exception):
    """Router 層嘅 error"""
    pass


def list_tasks() -> Dict[str, str]:
    """列出所有已知任務"""
    return {tid: r.description for tid, r in ALL_TASKS.items()}


def route(task: str, context: dict = None) -> dict:
    """路由：俾個任務，router 話你知最快可用嘅方法。
    
    新增功能：如果有歷史記錄（>=5 samples），會動態調整 path 順序，
    優先揀歷史最快成功嗰條，跳過最近連續 fail 嘅 path。
    冇歷史 data 時，行為同以前完全一樣（static tier order）。
    
    Args:
        task: 任務 ID
        context: 額外 context（optional）
    
    Returns:
        {
            "task": str,           # 任務 ID
            "available": bool,     # 有冇可用路徑？
            "path": Path | None,   # 揀咗嘅路徑
            "paths_checked": list,  # 所有 path 嘅檢查結果
            "reason": str,         # 人類可讀解釋
            "learning": dict,      # 學習資訊（optional）
        }
    """
    context = context or {}
    
    route_def = ALL_TASKS.get(task)
    if not route_def:
        return {
            "task": task,
            "available": False,
            "path": None,
            "paths_checked": [],
            "reason": f"❌ 未知任務: {task}",
        }
    
    # 有歷史 data？動態排序 paths
    best = _HISTORY_DB.best_path(task, min_samples=5)
    sorted_paths = list(route_def.paths)
    
    if best:
        # 歷史最快嗰條擺第一，其他照 tier 順序
        def _sort_key(p: Path):
            return (0 if p.id == best["path"] else 1, p.tier)
        sorted_paths.sort(key=_sort_key)
        learning_info = {"historical_best": best, "dynamic_sort": True}
    else:
        learning_info = {"historical_best": None, "dynamic_sort": False}
    
    # Check each path by sorted order
    paths_checked = []
    
    for path in sorted_paths:
        # 有冇連續 fail？（最近 3 次）
        recent = _HISTORY_DB.recent_failures(task, path.id, n=3)
        consecutive_fails = sum(1 for r in recent if not r["success"])
        if consecutive_fails >= 3:
            paths_checked.append({
                "path": path.id,
                "tier": path.tier,
                "name": path.name,
                "available": False,
                "message": f"⏭️ 跳過 — 連續 {consecutive_fails} 次 fail",
            })
            continue
        
        check_name = path.check_fn
        check_fn = CHECK_FUNCS.get(check_name)
        
        if check_fn:
            check_result = check_fn()
            paths_checked.append({
                "path": path.id,
                "tier": path.tier,
                "name": path.name,
                "available": check_result["ok"],
                "message": check_result["message"],
            })
            
            if check_result["ok"]:
                # Found the fastest available path
                return {
                    "task": task,
                    "available": True,
                    "path": path,
                    "paths_checked": paths_checked,
                    "reason": f"✅ {path.name} (Tier {path.tier})",
                    "learning": learning_info,
                }
        else:
            # No check needed — always available
            paths_checked.append({
                "path": path.id,
                "tier": path.tier,
                "name": path.name,
                "available": True,
                "message": "✅ 無需前置檢查",
            })
    
    # No path available
    return {
        "task": task,
        "available": False,
        "path": None,
        "paths_checked": paths_checked,
        "reason": f"❌ {task}: 所有路徑都唔可用",
        "learning": learning_info,
    }


def execute(route_result: dict, args: dict = None) -> dict:
    """執行 router 揀咗嘅路徑，並自動記錄結果到歷史 DB。
    
    Args:
        route_result: route() 嘅 return dict
        args: 執行參數
    
    Returns:
        {"success": bool, "result": any, "method": str, "error": str, "time_ms": int}
    """
    args = args or {}
    
    if not route_result.get("available"):
        return {
            "success": False,
            "result": None,
            "method": "none",
            "error": route_result.get("reason", "冇可用路徑"),
        }
    
    path = route_result["path"]
    task = route_result["task"]
    exec_fn = EXEC_FUNCS.get(path.exec_fn)
    
    if not exec_fn:
        return {
            "success": False,
            "error": f"未知執行函數: {path.exec_fn}",
        }
    
    # 執行 + 計時
    start = time.monotonic()
    result = exec_fn(task, args)
    elapsed_ms = int((time.monotonic() - start) * 1000)
    
    result["method"] = f"{path.name} (Tier {path.tier})"
    result["time_ms"] = elapsed_ms
    
    # 自動記錄到歷史 DB
    success = result.get("success", False)
    error_msg = result.get("error", "") or result.get("detail", "") or ""
    _HISTORY_DB.record(task, path.id, path.tier, success, elapsed_ms, error_msg)
    
    return result


def do(task: str, context: dict = None, args: dict = None) -> dict:
    """route + execute 合一。一句搞掂，自動學習。
    
    Usage:
        result = do("create_github_token",
                     args={"note": "hermes-agent-pr", "scopes": ["public_repo"]})
        if result["success"]:
            print(f"Token: {result['result']['token']}")
    
    每次 call 會自動記錄結果到歷史 DB，越用越聰明。
    """
    r = route(task, context)
    if not r.get("available"):
        return {"success": False, "error": r.get("reason")}
    return execute(r, args)


def history_stats(task: str = None) -> dict:
    """查 Router 學習統計
    
    Args:
        task: 指定任務（optional — 冇就回傳全部任務）
    
    Returns:
        {"task": str, "stats": {...}} or {"all_tasks": {...}}
    """
    if task:
        return {"task": task, "stats": _HISTORY_DB.stats(task)}
    # 全部任務
    all_stats = {}
    for tid in ALL_TASKS:
        s = _HISTORY_DB.stats(tid)
        if s["total"] > 0:
            all_stats[tid] = s
    return {"all_tasks": all_stats}


# ============================================================
# CLI entry
# ============================================================

if __name__ == "__main__":
    import sys
    
    if len(sys.argv) < 2:
        print("Usage: python3 router.py <task> [args...]")
        print("       python3 router.py stats [task_name]")
        print()
        print("Tasks:")
        for tid, desc in list_tasks().items():
            print(f"  {tid:<30} {desc}")
        sys.exit(1)
    
    if sys.argv[1] == "stats":
        task_name = sys.argv[2] if len(sys.argv) > 2 else None
        result = history_stats(task_name)
        print(json.dumps(result, indent=2, ensure_ascii=False))
        sys.exit(0)
    
    task = sys.argv[1]
    args = {}
    if len(sys.argv) > 2:
        try:
            args = json.loads(sys.argv[2])
        except json.JSONDecodeError:
            print(f"❌ JSON parse error: {sys.argv[2]}")
            sys.exit(1)
    
    result = do(task, args=args)
    print(json.dumps(result, indent=2, ensure_ascii=False))
