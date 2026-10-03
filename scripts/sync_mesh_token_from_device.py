#!/usr/bin/env python3
import subprocess
import json
import base64
import os
import sqlite3
import datetime
import tempfile
import requests

VPS_HOST = "127.0.0.1"
VPS_USER = "root"
VPS_PASS = ""
LOCAL_DB = "/home/ametist1337/projects/mesh-homework-bot/data/homeworks.db"
LOCAL_ENV = "/home/ametist1337/projects/mesh-homework-bot/.env"

def is_jwt_valid_and_not_expired(token: str, margin_seconds: int = 30) -> bool:
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
        return ("school.mos.ru" in iss) and (now < exp - margin_seconds)
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

def try_v3_refresh(refresh_token: str):
    if not refresh_token:
        return None, None
    url = "https://school.mos.ru/v3/token/refresh"
    headers = {
        "Content-Type": "application/x-www-form-urlencoded",
        "User-Agent": "okhttp/4.12.0"
    }
    try:
        resp = requests.post(url, data={"refresh_token": refresh_token.strip()}, headers=headers, timeout=10)
        if resp.status_code in (200, 201):
            data = resp.json()
            acc = data.get("access_token")
            ref = data.get("refresh_token")
            if acc:
                return acc, ref
    except Exception:
        pass
    return None, None

def run_sudir_sso_exchange(cookies_rows):
    s = requests.Session()
    s.headers.update({
        "User-Agent": "Mozilla/5.0 (Linux; Android 13; Pixel 7 Pro Build/TD1A.220804.031) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Mobile Safari/537.36"
    })

    for host_key, name, val in cookies_rows:
        if "mos.ru" in host_key:
            domain = host_key if host_key.startswith(".") else "." + host_key
            s.cookies.set(name, val, domain=domain)

    try:
        r1 = s.get("https://school.mos.ru/v3/auth/sudir/login", allow_redirects=True, timeout=15)
        import re
        m = re.search(r'url=([^"\' >]+)', r1.text, re.IGNORECASE)
        if not m:
            return None, None, []

        next_url = m.group(1).replace("&amp;", "&")
        if not next_url.startswith("http"):
            next_url = "https://login.mos.ru" + next_url

        r2 = s.get(next_url, allow_redirects=True, timeout=15)
        if r2.status_code != 200:
            return None, None, []

        acc = None
        ref = None
        for c in s.cookies:
            if c.name in ("aupd_token", "auth_token") and is_jwt_valid_and_not_expired(c.value):
                if acc is None or c.name == "aupd_token":
                    acc = c.value
            elif c.name in ("aupd_refresh_token", "refresh_token"):
                ref = c.value

        updated_cookies = []
        for c in s.cookies:
            updated_cookies.append((c.domain, c.name, c.value))

        return acc, ref, updated_cookies
    except Exception as e:
        print(f"Ошибка в run_sudir_sso_exchange: {e}")

    return None, None, []

def get_connected_devices():
    try:
        out = subprocess.check_output(["adb", "devices"], text=True)
        devs = []
        for line in out.splitlines()[1:]:
            parts = line.strip().split()
            if len(parts) >= 2 and parts[1] == "device":
                devs.append(parts[0])
        return devs
    except Exception:
        return []

def extract_from_device(device_ids=None):
    if not device_ids:
        device_ids = get_connected_devices()
    if not device_ids:
        device_ids = ["192.168.0.118:5555", "localhost:5555", "emulator-5554"]

    for dev in device_ids:
        cookies_rows = []
        candidate_refresh_tokens = []
        candidate_tokens = []

        # 1. Извлекаем Cookies из SQLite базы приложения ru.mesh.expressive или других
        with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as tmp_file:
            tmp_path = tmp_file.name

        try:
            packages = ["ru.mesh.expressive", "ru.mesh.client", "ru.mes.dnevnik"]
            pulled = False
            for pkg in packages:
                copy_cmd = ["adb", "-s", dev, "shell", f'su -c "cp /data/data/{pkg}/app_webview/Default/Cookies /data/local/tmp/Cookies && chmod 666 /data/local/tmp/Cookies"']
                subprocess.run(copy_cmd, capture_output=True, timeout=6)
                pull_cmd = ["adb", "-s", dev, "pull", "/data/local/tmp/Cookies", tmp_path]
                res_pull = subprocess.run(pull_cmd, capture_output=True, timeout=6)
                if res_pull.returncode == 0 and os.path.exists(tmp_path) and os.path.getsize(tmp_path) > 0:
                    pulled = True
                    break

            if pulled:
                conn = sqlite3.connect(tmp_path)
                c = conn.cursor()
                for host_key, name, val in c.execute("SELECT host_key, name, value FROM cookies"):
                    cookies_rows.append((host_key, name, val))
                    if name in ("aupd_refresh_token", "refresh_token"):
                        candidate_refresh_tokens.append(val)
                    elif name in ("aupd_token", "auth_token"):
                        candidate_tokens.append(val)
                conn.close()
        except Exception:
            pass
        finally:
            if os.path.exists(tmp_path):
                os.remove(tmp_path)

        # 2. Если есть Cookies, запускаем SSO обмен через сессию mos.ru
        if cookies_rows:
            acc, ref, updated_cookies = run_sudir_sso_exchange(cookies_rows)
            if acc:
                print("Успешное получение свежих токенов через СУДИР SSO flow!")
                return acc, ref, updated_cookies or cookies_rows

        # 3. Проверяем существующие refresh_token кандидаты
        for cr in set(candidate_refresh_tokens):
            acc, new_ref = try_v3_refresh(cr)
            if acc:
                print("Успешное прямое обновление токена через /v3/token/refresh!")
                return acc, new_ref or cr, cookies_rows

        # 4. Проверяем токены из shared_prefs/mesh_expressive_prefs.xml
        try:
            prefs_cmd = ["adb", "-s", dev, "shell", 'su -c "cat /data/data/ru.mesh.expressive/shared_prefs/mesh_expressive_prefs.xml 2>/dev/null"']
            res_prefs = subprocess.run(prefs_cmd, capture_output=True, text=True, timeout=6)
            if res_prefs.returncode == 0 and res_prefs.stdout:
                import re
                m_tok = re.search(r'<string name="auth_token">([^<]+)</string>', res_prefs.stdout)
                if m_tok:
                    t = m_tok.group(1).strip()
                    if is_jwt_valid_and_not_expired(t):
                        print("Успешно извлечен действующий auth_token из shared_prefs!")
                        return t, None, cookies_rows
        except Exception:
            pass

    return None, None, []

def update_local(token: str, refresh_token: str = None, cookies_str: str = None, cookies_json: str = None):
    now = datetime.datetime.now().isoformat()
    if os.path.exists(LOCAL_DB):
        conn = sqlite3.connect(LOCAL_DB)
        c = conn.cursor()
        c.execute("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('mesh_token', ?, ?)", (token, now))
        if refresh_token:
            c.execute("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('mesh_refresh_token', ?, ?)", (refresh_token, now))
        if cookies_str:
            c.execute("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('mesh_cookies', ?, ?)", (cookies_str, now))
        if cookies_json:
            c.execute("INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('mesh_cookies_json', ?, ?)", (cookies_json, now))
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

def update_vps(token: str, refresh_token: str = None, cookies_str: str = None, cookies_json: str = None):
    # Copy refresh_mesh_sso.py script to VPS
    local_script = os.path.join(os.path.dirname(os.path.abspath(__file__)), "refresh_mesh_sso.py")
    if os.path.exists(local_script):
        cmd_scp_script = [
            "sshpass", "-p", VPS_PASS,
            "scp", "-o", "StrictHostKeyChecking=no",
            local_script,
            f"{VPS_USER}@{VPS_HOST}:/root/projects/mesh-homework-bot/scripts/refresh_mesh_sso.py"
        ]
        subprocess.run(cmd_scp_script, timeout=15)

    sql = f"INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('mesh_token', '{token}', datetime('now'));"
    if refresh_token:
        sql += f" INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('mesh_refresh_token', '{refresh_token}', datetime('now'));"
    if cookies_str:
        clean_c = cookies_str.replace("'", "''")
        sql += f" INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES ('mesh_cookies', '{clean_c}', datetime('now'));"

    cmd_sql = [
        "sshpass", "-p", VPS_PASS,
        "ssh", "-o", "StrictHostKeyChecking=no", f"{VPS_USER}@{VPS_HOST}",
        f'sqlite3 /root/projects/mesh-homework-bot/data/homeworks.db "{sql}"'
    ]
    subprocess.run(cmd_sql, check=True, timeout=15)

    if cookies_json:
        with tempfile.NamedTemporaryFile(mode="w", suffix=".json", delete=False) as tmp_cj:
            tmp_cj.write(cookies_json)
            tmp_cj_path = tmp_cj.name
        try:
            cmd_scp_cj = [
                "sshpass", "-p", VPS_PASS,
                "scp", "-o", "StrictHostKeyChecking=no",
                tmp_cj_path,
                f"{VPS_USER}@{VPS_HOST}:/tmp/mesh_cookies_sync.json"
            ]
            subprocess.run(cmd_scp_cj, timeout=15)
            cmd_update_cj = [
                "sshpass", "-p", VPS_PASS,
                "ssh", "-o", "StrictHostKeyChecking=no", f"{VPS_USER}@{VPS_HOST}",
                "python3 -c 'import sqlite3; conn=sqlite3.connect(\"/root/projects/mesh-homework-bot/data/homeworks.db\"); c=conn.cursor(); c.execute(\"INSERT OR REPLACE INTO settings (key, value, updated_at) VALUES (\\x27mesh_cookies_json\\x27, ?, datetime(\\x27now\\x27))\", (open(\"/tmp/mesh_cookies_sync.json\").read(),)); conn.commit(); conn.close()'"
            ]
            subprocess.run(cmd_update_cj, timeout=15)
        finally:
            if os.path.exists(tmp_cj_path):
                os.remove(tmp_cj_path)

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
    token, refresh_token, cookies_rows = extract_from_device()
    if not token:
        print("Не найден активный токен на подключенных устройствах.")
        return

    exp = get_token_exp(token)
    now = datetime.datetime.now(datetime.timezone.utc).timestamp()
    hours_left = (exp - now) / 3600.0
    print(f"Получен токен: exp={exp} (осталось {hours_left:.1f} ч.), refresh_token={bool(refresh_token)}")

    cookies_str = "; ".join([f"{name}={val}" for host_key, name, val in cookies_rows if "mos.ru" in host_key and name in (
        "Ltpatoken2", "session-cookie", "bsh", "blc", "cTm", "cTmTgs", "obr_id", "user_login",
        "aupd_current_role", "profile_id", "student_person_id", "_bmus", "blrps", "origin", "oxxfgh"
    )])

    cookie_list = []
    for host_key, name, val in cookies_rows:
        cookie_list.append({
            "domain": host_key,
            "name": name,
            "value": val,
            "path": "/"
        })
    cookies_json = json.dumps(cookie_list) if cookie_list else None

    # 1. Обновляем локальные настройки
    update_local(token, refresh_token, cookies_str, cookies_json)
    print("Локальные DB и .env синхронизированы.")

    # 2. Проверяем состояние VPS
    vps_info = get_vps_token_info()
    vps_token = vps_info.get("mesh_token", "")
    vps_rt = vps_info.get("mesh_refresh_token", "")
    vps_valid = is_jwt_valid_and_not_expired(vps_token, margin_seconds=7200)

    # Проверяем, жив ли vps_rt
    vps_rt_alive = False
    if vps_rt and vps_rt == refresh_token:
        vps_rt_alive = True

    needs_vps_update = (not vps_valid) or (vps_token != token) or (refresh_token and not vps_rt_alive)
    if needs_vps_update:
        print(f"Обновление VPS (vps_valid={vps_valid}, token_match={vps_token == token}, rt_alive={vps_rt_alive}). Синхронизируем VPS...")
        update_vps(token, refresh_token, cookies_str, cookies_json)
        print("VPS успешно обновлен и mesh-homework-bot.service перезапущен!")
    else:
        print("VPS токен актуален и сервис работает штатно.")

if __name__ == "__main__":
    main()
