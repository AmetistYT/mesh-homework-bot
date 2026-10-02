#!/usr/bin/env python3
import subprocess
import json
import base64
import re
import os
import sqlite3
import datetime
import tempfile

VPS_HOST = "127.0.0.1"
VPS_USER = "root"
VPS_PASS = ""
LOCAL_DB = "/home/ametist1337/projects/mesh-homework-bot/data/homeworks.db"
LOCAL_ENV = "/home/ametist1337/projects/mesh-homework-bot/.env"

def is_jwt_valid_and_not_expired(token: str) -> bool:
    if not token or len(token) <= 20:
        return False
    parts = token.split(".")
    if len(parts) < 2:
        return False
    try:
        padded = parts[1] + "=" * ((4 - len(parts[1]) % 4) % 4)
        payload = json.loads(base64.urlsafe_b64decode(padded).decode("utf-8", errors="ignore"))
        exp = payload.get("exp", 0)
        iss = payload.get("iss", "")
        now = datetime.datetime.now(datetime.timezone.utc).timestamp()
        return ("school.mos.ru" in iss) and (now < exp - 30)
    except Exception:
        return False

def get_token_exp(token: str) -> int:
    try:
        parts = token.split(".")
        padded = parts[1] + "=" * ((4 - len(parts[1]) % 4) % 4)
        payload = json.loads(base64.urlsafe_b64decode(padded).decode("utf-8", errors="ignore"))
        return int(payload.get("exp", 0))
    except Exception:
        return 0

def extract_from_device(device_id="10.8.1.1:5555"):
    candidate_tokens = []
    candidate_refresh_tokens = []

    # 1. Check shared_prefs of mesh_expressive
    cmd_prefs = ["adb", "-s", device_id, "shell", 'su -c "cat /data/data/ru.mesh.expressive/shared_prefs/mesh_expressive_prefs.xml 2>/dev/null"']
    try:
        res = subprocess.run(cmd_prefs, capture_output=True, text=True, timeout=8)
        if res.returncode == 0:
            matches = re.findall(r'eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}', res.stdout)
            candidate_tokens.extend(matches)
    except Exception:
        pass

    # 2. Check cookies database of mesh_expressive
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp_file:
        tmp_path = tmp_file.name

    try:
        copy_cmd = ["adb", "-s", device_id, "shell", 'su -c "cp /data/data/ru.mesh.expressive/app_webview/Default/Cookies /data/local/tmp/Cookies && chmod 666 /data/local/tmp/Cookies"']
        subprocess.run(copy_cmd, capture_output=True, timeout=8)

        pull_cmd = ["adb", "-s", device_id, "pull", "/data/local/tmp/Cookies", tmp_path]
        res_pull = subprocess.run(pull_cmd, capture_output=True, timeout=8)

        if res_pull.returncode == 0 and os.path.exists(tmp_path) and os.path.getsize(tmp_path) > 0:
            conn = sqlite3.connect(tmp_path)
            c = conn.cursor()
            c.execute("SELECT name, value FROM cookies WHERE name IN ('aupd_token', 'auth_token', 'token', 'aupd_refresh_token', 'refresh_token');")
            for name, val in c.fetchall():
                if name in ('aupd_refresh_token', 'refresh_token'):
                    candidate_refresh_tokens.append(val)
                else:
                    candidate_tokens.append(val)
            conn.close()
    except Exception:
        pass
    finally:
        if os.path.exists(tmp_path):
            os.remove(tmp_path)

    valid_tokens = [t for t in set(candidate_tokens) if is_jwt_valid_and_not_expired(t)]
    if not valid_tokens:
        return None, None

    valid_tokens.sort(key=get_token_exp, reverse=True)
    best_access = valid_tokens[0]

    best_refresh = None
    if candidate_refresh_tokens:
        candidate_refresh_tokens.sort(key=get_token_exp, reverse=True)
        best_refresh = candidate_refresh_tokens[0]

    return best_access, best_refresh

def update_local(token: str, refresh_token: str = None):
    now = datetime.datetime.now().isoformat()
    if os.path.exists(LOCAL_DB):
        conn = sqlite3.connect(LOCAL_DB)
        c = conn.cursor()
        c.execute("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('mesh_token', ?, ?)", (token, now))
        if refresh_token:
            c.execute("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('mesh_refresh_token', ?, ?)", (refresh_token, now))
        conn.commit()
        conn.close()

    if os.path.exists(LOCAL_ENV):
        with open(LOCAL_ENV, "r", encoding="utf-8") as f:
            lines = f.readlines()
        has_refresh = False
        new_lines = []
        for l in lines:
            if l.startswith("MESH_JWT_TOKEN="):
                new_lines.append(f"MESH_JWT_TOKEN={token}\n")
            elif l.startswith("MESH_REFRESH_TOKEN="):
                has_refresh = True
                if refresh_token:
                    new_lines.append(f"MESH_REFRESH_TOKEN={refresh_token}\n")
                else:
                    new_lines.append(l)
            else:
                new_lines.append(l)
        if not has_refresh and refresh_token:
            new_lines.append(f"MESH_REFRESH_TOKEN={refresh_token}\n")
        with open(LOCAL_ENV, "w", encoding="utf-8") as f:
            f.writelines(new_lines)

def get_vps_token_info():
    cmd = [
        "sshpass", "-p", VPS_PASS,
        "ssh", "-o", "StrictHostKeyChecking=no", f"{VPS_USER}@{VPS_HOST}",
        "sqlite3 /root/projects/mesh-homework-bot/data/homeworks.db \"SELECT key, value FROM settings WHERE key IN ('mesh_token', 'mesh_refresh_token');\""
    ]
    try:
        res = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
        data = {}
        if res.returncode == 0:
            for line in res.stdout.splitlines():
                if "|" in line:
                    k, v = line.split("|", 1)
                    data[k] = v
        return data
    except Exception:
        return {}

def update_vps(token: str, refresh_token: str = None):
    sql = f"INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('mesh_token', '{token}', datetime('now'));"
    if refresh_token:
        sql += f" INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('mesh_refresh_token', '{refresh_token}', datetime('now'));"

    cmd_sql = [
        "sshpass", "-p", VPS_PASS,
        "ssh", "-o", "StrictHostKeyChecking=no", f"{VPS_USER}@{VPS_HOST}",
        f'sqlite3 /root/projects/mesh-homework-bot/data/homeworks.db "{sql}"'
    ]
    subprocess.run(cmd_sql, check=True, timeout=15)

    cmd_env = [
        "sshpass", "-p", VPS_PASS,
        "ssh", "-o", "StrictHostKeyChecking=no", f"{VPS_USER}@{VPS_HOST}",
        f"sed -i 's|^MESH_JWT_TOKEN=.*|MESH_JWT_TOKEN={token}|' /root/projects/mesh-homework-bot/.env"
    ]
    subprocess.run(cmd_env, check=True, timeout=15)

    if refresh_token:
        cmd_rt_env = [
            "sshpass", "-p", VPS_PASS,
            "ssh", "-o", "StrictHostKeyChecking=no", f"{VPS_USER}@{VPS_HOST}",
            f"grep -q '^MESH_REFRESH_TOKEN=' /root/projects/mesh-homework-bot/.env && sed -i 's|^MESH_REFRESH_TOKEN=.*|MESH_REFRESH_TOKEN={refresh_token}|' /root/projects/mesh-homework-bot/.env || echo 'MESH_REFRESH_TOKEN={refresh_token}' >> /root/projects/mesh-homework-bot/.env"
        ]
        subprocess.run(cmd_rt_env, check=True, timeout=15)

    cmd_restart = [
        "sshpass", "-p", VPS_PASS,
        "ssh", "-o", "StrictHostKeyChecking=no", f"{VPS_USER}@{VPS_HOST}",
        "systemctl restart mesh-homework-bot.service"
    ]
    subprocess.run(cmd_restart, check=True, timeout=15)

def main():
    token, refresh_token = extract_from_device()
    if not token:
        print("No valid active token found on device.")
        return

    exp = get_token_exp(token)
    print(f"Extracted valid token, exp timestamp: {exp}")
    
    cur_token = None
    if os.path.exists(LOCAL_DB):
        try:
            conn = sqlite3.connect(LOCAL_DB)
            c = conn.cursor()
            c.execute("SELECT value FROM settings WHERE key='mesh_token'")
            row = c.fetchone()
            if row:
                cur_token = row[0]
            conn.close()
        except Exception:
            pass

    needs_local_update = (cur_token != token)
    if needs_local_update:
        print("Local token differs or missing. Updating local...")
        update_local(token, refresh_token)
        print("Local updated.")

    vps_info = get_vps_token_info()
    vps_token = vps_info.get("mesh_token", "")
    vps_is_valid = is_jwt_valid_and_not_expired(vps_token)

    if not vps_is_valid or vps_token != token:
        print(f"VPS token needs update (valid={vps_is_valid}, matches={vps_token == token}). Updating VPS...")
        update_vps(token, refresh_token)
        print("VPS updated and mesh-homework-bot.service restarted successfully!")
    else:
        print("VPS token is valid and up to date.")

if __name__ == "__main__":
    main()
