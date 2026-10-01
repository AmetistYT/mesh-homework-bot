#!/usr/bin/env python3
import subprocess
import json
import base64
import re
import os
import sqlite3
import datetime

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

    # 1. Check shared_prefs of mesh_expressive
    cmd = ["adb", "-s", device_id, "shell", 'su -c "cat /data/data/ru.mesh.expressive/shared_prefs/mesh_expressive_prefs.xml 2>/dev/null"']
    try:
        res = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
        if res.returncode == 0:
            matches = re.findall(r'eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}', res.stdout)
            candidate_tokens.extend(matches)
    except Exception:
        pass

    # 2. Check cookies database of mesh_expressive
    cmd_cookies = ["adb", "-s", device_id, "shell", 'su -c "sqlite3 /data/data/ru.mesh.expressive/app_webview/Default/Cookies \\\"SELECT value FROM cookies WHERE name IN (\'auth_token\', \'aupd_token\', \'token\');\\\" 2>/dev/null"']
    try:
        res_c = subprocess.run(cmd_cookies, capture_output=True, text=True, timeout=10)
        if res_c.returncode == 0:
            matches_c = re.findall(r'eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}', res_c.stdout)
            candidate_tokens.extend(matches_c)
    except Exception:
        pass

    valid_tokens = [t for t in set(candidate_tokens) if is_jwt_valid_and_not_expired(t)]
    if not valid_tokens:
        return None

    valid_tokens.sort(key=get_token_exp, reverse=True)
    return valid_tokens[0]

def update_local(token: str):
    now = datetime.datetime.now().isoformat()
    if os.path.exists(LOCAL_DB):
        conn = sqlite3.connect(LOCAL_DB)
        c = conn.cursor()
        c.execute("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('mesh_token', ?, ?)", (token, now))
        conn.commit()
        conn.close()

    if os.path.exists(LOCAL_ENV):
        with open(LOCAL_ENV, "r", encoding="utf-8") as f:
            lines = f.readlines()
        with open(LOCAL_ENV, "w", encoding="utf-8") as f:
            for l in lines:
                if l.startswith("MESH_JWT_TOKEN="):
                    f.write(f"MESH_JWT_TOKEN={token}\n")
                else:
                    f.write(l)

def update_vps(token: str):
    now = datetime.datetime.now().isoformat()
    remote_script = f"""
import sqlite3
token = "{token}"
now = "{now}"
conn = sqlite3.connect("/root/projects/mesh-homework-bot/data/homeworks.db")
c = conn.cursor()
c.execute("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('mesh_token', ?, ?)", (token, now))
conn.commit()
conn.close()
"""
    cmd_ssh = [
        "sshpass", "-p", VPS_PASS,
        "ssh", "-o", "StrictHostKeyChecking=no", f"{VPS_USER}@{VPS_HOST}",
        f"python3 -c '{remote_script}' && sed -i 's|^MESH_JWT_TOKEN=.*|MESH_JWT_TOKEN={token}|' /root/projects/mesh-homework-bot/.env"
    ]
    subprocess.run(cmd_ssh, check=True, timeout=15)

def main():
    token = extract_from_device()
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

    if cur_token == token:
        print("Token on device matches current active token. Up to date.")
        return

    print("New token detected! Updating local and VPS...")
    update_local(token)
    update_vps(token)
    print("Token synchronized successfully to local and VPS!")

if __name__ == "__main__":
    main()
