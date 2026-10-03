#!/usr/bin/env python3
import sys
import os
import json
import sqlite3
import datetime
import base64
import requests

def get_paths():
    base_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    db_path = os.path.join(base_dir, "data", "homeworks.db")
    env_path = os.path.join(base_dir, ".env")
    return db_path, env_path

def is_jwt_valid(token: str, margin_seconds: int = 60) -> bool:
    if not token or len(token) <= 20:
        return False
    parts = token.split(".")
    if len(parts) < 2:
        return False
    try:
        p = parts[1] + "=" * ((4 - len(parts[1]) % 4) % 4)
        data = json.loads(base64.urlsafe_b64decode(p).decode("utf-8", errors="ignore"))
        exp = data.get("exp", 0)
        now = datetime.datetime.now(datetime.timezone.utc).timestamp()
        return ("school.mos.ru" in data.get("iss", "")) and (now < exp - margin_seconds)
    except Exception:
        return False

def get_jwt_exp(token: str) -> int:
    try:
        p = token.split(".")[1]
        p += "=" * ((4 - len(p) % 4) % 4)
        data = json.loads(base64.urlsafe_b64decode(p).decode("utf-8", errors="ignore"))
        return int(data.get("exp", 0))
    except Exception:
        return 0

def update_env_file(env_path: str, token: str, refresh_token: str = None):
    if not os.path.exists(env_path):
        return
    with open(env_path, "r", encoding="utf-8") as f:
        lines = f.readlines()
    has_jwt = False
    has_rt = False
    new_lines = []
    for l in lines:
        if l.startswith("MESH_JWT_TOKEN="):
            new_lines.append(f"MESH_JWT_TOKEN={token}\n")
            has_jwt = True
        elif l.startswith("MESH_REFRESH_TOKEN="):
            if refresh_token:
                new_lines.append(f"MESH_REFRESH_TOKEN={refresh_token}\n")
            else:
                new_lines.append(l)
            has_rt = True
        else:
            new_lines.append(l)
    if not has_jwt:
        new_lines.append(f"MESH_JWT_TOKEN={token}\n")
    if not has_rt and refresh_token:
        new_lines.append(f"MESH_REFRESH_TOKEN={refresh_token}\n")
    with open(env_path, "w", encoding="utf-8") as f:
        f.writelines(new_lines)

def run_sso_refresh():
    db_path, env_path = get_paths()
    if not os.path.exists(db_path):
        return {"status": "error", "message": f"Database file not found: {db_path}"}

    conn = sqlite3.connect(db_path)
    c = conn.cursor()
    c.execute("SELECT value FROM settings WHERE key='mesh_cookies_json'")
    row_json = c.fetchone()
    cookies_json_str = row_json[0] if row_json else None

    c.execute("SELECT value FROM settings WHERE key='mesh_refresh_token'")
    row_rt = c.fetchone()
    stored_refresh_token = row_rt[0] if row_rt else None
    conn.close()

    s = requests.Session()
    s.headers.update({
        "User-Agent": "Mozilla/5.0 (Linux; Android 13; Pixel 7 Pro Build/TD1A.220804.031) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Mobile Safari/537.36"
    })

    cookies_loaded = False
    if cookies_json_str:
        try:
            cookie_list = json.loads(cookies_json_str)
            for ck in cookie_list:
                s.cookies.set(ck["name"], ck["value"], domain=ck.get("domain", ".mos.ru"), path=ck.get("path", "/"))
            cookies_loaded = True
        except Exception:
            pass

    if not cookies_loaded and cookies_legacy_str:
        for part in cookies_legacy_str.split(";"):
            if "=" in part:
                k, v = part.strip().split("=", 1)
                s.cookies.set(k, v, domain=".mos.ru")
                s.cookies.set(k, v, domain="login.mos.ru")
                s.cookies.set(k, v, domain="school.mos.ru")
        cookies_loaded = True

    if not cookies_loaded and not stored_refresh_token:
        return {"status": "error", "message": "No cookies or refresh token found in database settings"}

    try:
        import re
        r1 = s.get("https://school.mos.ru/v3/auth/sudir/login", allow_redirects=True, timeout=15)
        m = re.search(r'url=([^"\' >]+)', r1.text, re.IGNORECASE)
        if m:
            next_url = m.group(1).replace("&amp;", "&")
            if not next_url.startswith("http"):
                next_url = "https://login.mos.ru" + next_url
            r2 = s.get(next_url, allow_redirects=True, timeout=15)

        new_token = None
        new_refresh = None
        for ck in s.cookies:
            if ck.name in ("aupd_token", "auth_token", "token") and is_jwt_valid(ck.value):
                if new_token is None or ck.name == "aupd_token":
                    new_token = ck.value
            elif ck.name in ("aupd_refresh_token", "refresh_token"):
                new_refresh = ck.value

        # Резерв: если SSO через куки не выдал JWT, пробуем /v3/token/refresh
        if not new_token and (new_refresh or stored_refresh_token):
            cand_rt = new_refresh or stored_refresh_token
            try:
                ref_resp = requests.post(
                    "https://school.mos.ru/v3/token/refresh",
                    data={"refresh_token": cand_rt.strip()},
                    headers={"Content-Type": "application/x-www-form-urlencoded", "User-Agent": "okhttp/4.12.0"},
                    timeout=10
                )
                if ref_resp.status_code in (200, 201):
                    ref_data = ref_resp.json()
                    tok_cand = ref_data.get("access_token")
                    if tok_cand and is_jwt_valid(tok_cand):
                        new_token = tok_cand
                        new_refresh = ref_data.get("refresh_token") or cand_rt
            except Exception:
                pass

        if not new_token:
            return {"status": "error", "message": "SSO exchange and v3 refresh did not produce a valid JWT token"}

        all_cookies = []
        flat_parts = []
        for ck in s.cookies:
            all_cookies.append({
                "domain": ck.domain,
                "name": ck.name,
                "value": ck.value,
                "path": ck.path,
                "secure": ck.secure
            })
            if "mos.ru" in ck.domain:
                flat_parts.append(f"{ck.name}={ck.value}")

        now_iso = datetime.datetime.now().isoformat()
        conn = sqlite3.connect(db_path)
        c = conn.cursor()
        c.execute("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('mesh_token', ?, ?)", (new_token, now_iso))
        if new_refresh:
            c.execute("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('mesh_refresh_token', ?, ?)", (new_refresh, now_iso))
        c.execute("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('mesh_cookies_json', ?, ?)", (json.dumps(all_cookies), now_iso))
        c.execute("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('mesh_cookies', ?, ?)", ("; ".join(flat_parts), now_iso))
        conn.commit()
        conn.close()

        update_env_file(env_path, new_token, new_refresh)

        return {
            "status": "ok",
            "token": new_token,
            "refresh_token": new_refresh,
            "exp": get_jwt_exp(new_token)
        }

    except Exception as e:
        return {"status": "error", "message": f"Exception during SSO refresh: {str(e)}"}

if __name__ == "__main__":
    result = run_sso_refresh()
    print(json.dumps(result))
    if result.get("status") != "ok":
        sys.exit(1)
