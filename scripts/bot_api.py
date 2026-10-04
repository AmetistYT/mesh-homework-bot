#!/usr/bin/env python3
"""
Инструмент и API бота для анализа чата, статистики и взаимодействия с Telegram API.
Может использоваться как библиотека (`import scripts.bot_api as bot_api`)
или как CLI:
    python3 scripts/bot_api.py top [limit] [--chat-id ID] [--days N]
    python3 scripts/bot_api.py stats [--chat-id ID]
    python3 scripts/bot_api.py search "текст" [--chat-id ID]
    python3 scripts/bot_api.py sql "SELECT ..."
    python3 scripts/bot_api.py send "текст сообщения" [--chat-id ID]
"""

import os
import sys
import json
import sqlite3
import argparse
import urllib.request
import urllib.error
from pathlib import Path
from datetime import datetime, timedelta

def get_db_path() -> Path:
    env_path = os.environ.get("DB_PATH")
    if env_path:
        p = Path(env_path)
        if p.exists():
            return p
    # Стандартные пути
    candidates = [
        Path("data/homeworks.db"),
        Path("/root/projects/mesh-homework-bot/data/homeworks.db"),
        Path.home() / "projects/mesh-homework-bot/data/homeworks.db",
    ]
    for c in candidates:
        if c.exists():
            return c
    return Path("data/homeworks.db")

def get_bot_token() -> str:
    token = os.environ.get("TELEGRAM_BOT_TOKEN", "").strip()
    if token:
        return token
    # Попытка прочитать из .env
    for env_file in [Path(".env"), Path("/root/projects/mesh-homework-bot/.env")]:
        if env_file.exists():
            try:
                for line in env_file.read_text(encoding="utf-8").splitlines():
                    if line.startswith("TELEGRAM_BOT_TOKEN="):
                        return line.split("=", 1)[1].strip().strip('"').strip("'")
            except Exception:
                pass
    return ""

def get_default_chat_id() -> str:
    cid = os.environ.get("TELEGRAM_CHAT_ID", "").strip()
    if cid:
        return cid
    for env_file in [Path(".env"), Path("/root/projects/mesh-homework-bot/.env")]:
        if env_file.exists():
            try:
                for line in env_file.read_text(encoding="utf-8").splitlines():
                    if line.startswith("TELEGRAM_CHANNEL_ID="):
                        return line.split("=", 1)[1].strip().strip('"').strip("'")
            except Exception:
                pass
    return "-1004469624876"

def get_db_connection() -> sqlite3.Connection:
    db_file = get_db_path()
    conn = sqlite3.connect(str(db_file))
    conn.row_factory = sqlite3.Row
    return conn

def get_top_senders(chat_id: str = None, limit: int = 20, days: int = None) -> list:
    """
    Возвращает топ участников по количеству сообщений.
    Включает user_id, имя, @username, приписку (custom_title) и количество сообщений.
    """
    conn = get_db_connection()
    chat_filter = ""
    params = []
    
    if chat_id:
        chat_filter += " WHERE m.chat_id = ?"
        params.append(str(chat_id))
        
    if days:
        since_ts = int((datetime.utcnow() - timedelta(days=days)).timestamp())
        if chat_filter:
            chat_filter += " AND m.timestamp >= ?"
        else:
            chat_filter += " WHERE m.timestamp >= ?"
        params.append(since_ts)

    query = f"""
        SELECT 
            m.user_id,
            COALESCE(u.custom_title, '') as custom_title,
            COALESCE(u.username, m.username, '') as username,
            COALESCE(u.first_name || CASE WHEN u.last_name IS NOT NULL THEN ' ' || u.last_name ELSE '' END, m.full_name) as display_name,
            COUNT(*) as message_count,
            MIN(m.timestamp) as first_seen,
            MAX(m.timestamp) as last_seen
        FROM chat_messages m
        LEFT JOIN seen_users u ON m.user_id = u.user_id
        {chat_filter}
        GROUP BY m.user_id
        ORDER BY message_count DESC
        LIMIT ?
    """
    params.append(limit)
    cursor = conn.cursor()
    try:
        cursor.execute(query, params)
        rows = cursor.fetchall()
    except sqlite3.OperationalError:
        rows = []
    
    res = []
    for r in rows:
        res.append({
            "user_id": r["user_id"],
            "custom_title": r["custom_title"],
            "username": r["username"],
            "display_name": r["display_name"],
            "message_count": r["message_count"],
            "first_seen": r["first_seen"],
            "last_seen": r["last_seen"]
        })
    conn.close()
    return res

def format_top_senders(top: list, days: int = None) -> str:
    """Форматирует топ участников в красивый текст для Telegram Markdown."""
    if not top:
        return "В базе данных пока нет сообщений для составления топа."
    
    header = f"*Топ участников по сообщениям"
    if days:
        header += f" (за последние {days} дн.)*:\n"
    else:
        header += f" (за всё время)*:\n"

    lines = [header]
    for idx, user in enumerate(top, 1):
        name = user["display_name"]
        title = f" [{user['custom_title']}]" if user["custom_title"] else ""
        un = f" (@{user['username']})" if user["username"] else ""
        cnt = user["message_count"]
        lines.append(f"{idx}. *{name}*{title}{un} — `{cnt}` сообщ.")

    return "\n".join(lines)

def get_chat_stats(chat_id: str = None) -> dict:
    """Возвращает общую статистику переписки чата."""
    conn = get_db_connection()
    c = conn.cursor()
    params = [str(chat_id)] if chat_id else []
    where = "WHERE chat_id = ?" if chat_id else ""
    
    try:
        c.execute(f"SELECT COUNT(*), COUNT(DISTINCT user_id), MIN(timestamp), MAX(timestamp) FROM chat_messages {where}", params)
        row = c.fetchone()
        total_msgs = row[0] or 0
        unique_users = row[1] or 0
        first_ts = row[2]
        last_ts = row[3]
        
        c.execute(f"""
            SELECT date(datetime(timestamp, 'unixepoch')), COUNT(*) 
            FROM chat_messages {where} 
            GROUP BY date(datetime(timestamp, 'unixepoch')) 
            ORDER BY 2 DESC LIMIT 1
        """, params)
        peak_row = c.fetchone()
        peak_date = peak_row[0] if peak_row else "—"
        peak_count = peak_row[1] if peak_row else 0
    except sqlite3.OperationalError:
        total_msgs = 0
        unique_users = 0
        first_ts = None
        last_ts = None
        peak_date = "—"
        peak_count = 0

    conn.close()
    return {
        "total_messages": total_msgs,
        "unique_users": unique_users,
        "first_timestamp": first_ts,
        "last_timestamp": last_ts,
        "peak_date": peak_date,
        "peak_count": peak_count
    }

def search_messages(query: str, chat_id: str = None, user: str = None, limit: int = 25) -> list:
    """Поиск сообщений по подстроке текста и/или пользователю."""
    conn = get_db_connection()
    c = conn.cursor()
    conditions = ["m.text LIKE ?"]
    params = [f"%{query}%"]
    
    if chat_id:
        conditions.append("m.chat_id = ?")
        params.append(str(chat_id))
    if user:
        clean_user = user.lstrip("@").lower()
        conditions.append("(LOWER(m.username) = ? OR LOWER(m.full_name) LIKE ? OR m.user_id = ?)")
        params.extend([clean_user, f"%{clean_user}%", int(clean_user) if clean_user.isdigit() else 0])
        
    where = "WHERE " + " AND ".join(conditions)
    sql = f"""
        SELECT m.id, m.message_id, m.user_id, COALESCE(u.custom_title, '') as custom_title,
               m.username, m.full_name, m.text, m.reply_to_message_id, m.timestamp,
               datetime(m.timestamp, 'unixepoch', 'localtime') as dt
        FROM chat_messages m
        LEFT JOIN seen_users u ON m.user_id = u.user_id
        {where}
        ORDER BY m.timestamp DESC
        LIMIT ?
    """
    params.append(limit)
    try:
        c.execute(sql, params)
        rows = [dict(r) for r in c.fetchall()]
    except Exception as e:
        rows = [{"error": str(e)}]
    conn.close()
    return rows

def get_user_history(user: str, chat_id: str = None, limit: int = 30) -> list:
    """Получает последние сообщения указанного участника."""
    conn = get_db_connection()
    c = conn.cursor()
    clean_user = user.lstrip("@").lower()
    conditions = ["(LOWER(m.username) = ? OR LOWER(m.full_name) LIKE ? OR m.user_id = ?)"]
    params = [clean_user, f"%{clean_user}%", int(clean_user) if clean_user.isdigit() else 0]
    
    if chat_id:
        conditions.append("m.chat_id = ?")
        params.append(str(chat_id))
        
    where = "WHERE " + " AND ".join(conditions)
    sql = f"""
        SELECT m.id, m.message_id, m.user_id, COALESCE(u.custom_title, '') as custom_title,
               m.username, m.full_name, m.text, m.reply_to_message_id, m.timestamp,
               datetime(m.timestamp, 'unixepoch', 'localtime') as dt
        FROM chat_messages m
        LEFT JOIN seen_users u ON m.user_id = u.user_id
        {where}
        ORDER BY m.timestamp DESC
        LIMIT ?
    """
    params.append(limit)
    try:
        c.execute(sql, params)
        rows = [dict(r) for r in c.fetchall()]
    except Exception as e:
        rows = [{"error": str(e)}]
    conn.close()
    return rows

def import_export_json(json_path: str, chat_id: str = "-1004469624876") -> int:
    """Импортирует историю сообщений из result.json Telegram Desktop / AyuGram в chat_messages."""
    p = Path(json_path)
    if not p.exists():
        raise FileNotFoundError(f"Файл {json_path} не найден")
    with open(p, "r", encoding="utf-8") as f:
        data = json.load(f)
        
    msgs = data.get("messages", [])
    conn = get_db_connection()
    c = conn.cursor()
    
    inserted = 0
    now_str = datetime.utcnow().strftime("%Y-%m-%d %H:%M:%S")
    for m in msgs:
        if m.get("type") != "message":
            continue
        msg_id = m.get("id")
        if not msg_id:
            continue
            
        from_id_raw = m.get("from_id") or ""
        if from_id_raw.startswith("user"):
            user_id = int(from_id_raw.replace("user", ""))
        elif from_id_raw.startswith("channel"):
            user_id = int(from_id_raw.replace("channel", ""))
        elif str(from_id_raw).isdigit():
            user_id = int(from_id_raw)
        else:
            user_id = 0
            
        full_name = m.get("from") or "Пользователь"
        username = None
        
        txt_val = m.get("text")
        if isinstance(txt_val, list):
            parts = []
            for part in txt_val:
                if isinstance(part, str):
                    parts.append(part)
                elif isinstance(part, dict):
                    parts.append(part.get("text", ""))
            text = "".join(parts).strip()
        elif isinstance(txt_val, str):
            text = txt_val.strip()
        else:
            text = ""
            
        if not text:
            if m.get("photo"):
                text = "[Фото]"
            elif m.get("media_type") == "sticker":
                text = "[Стикер]"
            elif m.get("media_type") == "video_file":
                text = "[Видео]"
            elif m.get("media_type") == "voice_message":
                text = "[Голосовое сообщение]"
            elif m.get("media_type") == "audio_file":
                text = "[Аудио]"
            else:
                text = "[Медиа]"
                
        reply_to_id = m.get("reply_to_message_id")
        timestamp = int(m.get("date_unixtime") or 0)
        created_at = m.get("date") or now_str
        
        try:
            c.execute("""
                INSERT OR IGNORE INTO chat_messages (
                    chat_id, message_id, user_id, username, full_name, text,
                    reply_to_message_id, timestamp, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            """, (chat_id, msg_id, user_id, username, full_name, text, reply_to_id, timestamp, created_at))
            if c.rowcount > 0:
                inserted += 1
        except Exception:
            pass
            
    conn.commit()
    conn.close()
    return inserted

def execute_sql(query: str, params: list = None) -> list:
    """Выполняет произвольный SQL-запрос на чтение из БД."""
    conn = get_db_connection()
    cursor = conn.cursor()
    cursor.execute(query, params or [])
    rows = cursor.fetchall()
    res = [dict(r) for r in rows]
    conn.close()
    return res

def send_telegram_message(text: str, chat_id: str = None, reply_to_message_id: int = None, parse_mode: str = "Markdown") -> dict:
    """Отправляет сообщение напрямую в Telegram через Bot API."""
    token = get_bot_token()
    if not token:
        raise ValueError("TELEGRAM_BOT_TOKEN не найден в окружении или .env")
    
    target_chat = chat_id or get_default_chat_id()
    url = f"https://api.telegram.org/bot{token}/sendMessage"
    payload = {
        "chat_id": target_chat,
        "text": text,
        "parse_mode": parse_mode
    }
    if reply_to_message_id:
        payload["reply_to_message_id"] = reply_to_message_id

    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(url, data=data, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=10) as resp:
        return json.loads(resp.read().decode("utf-8"))

def send_telegram_photo(photo_path: str, caption: str = None, chat_id: str = None, reply_to_message_id: int = None) -> dict:
    """Отправляет фото напрямую в Telegram через Bot API multipart/form-data."""
    token = get_bot_token()
    if not token:
        raise ValueError("TELEGRAM_BOT_TOKEN не найден в окружении или .env")
    
    target_chat = chat_id or get_default_chat_id()
    url = f"https://api.telegram.org/bot{token}/sendPhoto"
    
    boundary = "----WebKitFormBoundary" + os.urandom(16).hex()
    body = bytearray()
    
    # chat_id
    body.extend(f"--{boundary}\r\n".encode("utf-8"))
    body.extend(b'Content-Disposition: form-data; name="chat_id"\r\n\r\n')
    body.extend(f"{target_chat}\r\n".encode("utf-8"))
    
    # caption
    if caption:
        body.extend(f"--{boundary}\r\n".encode("utf-8"))
        body.extend(b'Content-Disposition: form-data; name="caption"\r\n\r\n')
        body.extend(f"{caption}\r\n".encode("utf-8"))
        
    # reply_to_message_id
    if reply_to_message_id:
        body.extend(f"--{boundary}\r\n".encode("utf-8"))
        body.extend(b'Content-Disposition: form-data; name="reply_to_message_id"\r\n\r\n')
        body.extend(f"{reply_to_message_id}\r\n".encode("utf-8"))
        
    # photo file
    p = Path(photo_path)
    if not p.exists():
        raise FileNotFoundError(f"Файл {photo_path} не найден")
        
    filename = p.name
    content_type = "image/jpeg" if filename.lower().endswith((".jpg", ".jpeg")) else "image/png"
    body.extend(f"--{boundary}\r\n".encode("utf-8"))
    body.extend(f'Content-Disposition: form-data; name="photo"; filename="{filename}"\r\n'.encode("utf-8"))
    body.extend(f"Content-Type: {content_type}\r\n\r\n".encode("utf-8"))
    body.extend(p.read_bytes())
    body.extend(b"\r\n")
    
    body.extend(f"--{boundary}--\r\n".encode("utf-8"))
    
    req = urllib.request.Request(
        url,
        data=bytes(body),
        headers={"Content-Type": f"multipart/form-data; boundary={boundary}"}
    )
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.loads(resp.read().decode("utf-8"))

def telegram_api_call(method: str, params: dict = None) -> dict:
    """Выполняет произвольный запрос к Telegram Bot API."""
    token = get_bot_token()
    if not token:
        raise ValueError("TELEGRAM_BOT_TOKEN не задан")
    
    url = f"https://api.telegram.org/bot{token}/{method}"
    data = json.dumps(params or {}).encode("utf-8")
    req = urllib.request.Request(url, data=data, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=15) as resp:
        return json.loads(resp.read().decode("utf-8"))

def main():
    parser = argparse.ArgumentParser(description="Bot API & Database CLI Tool")
    subparsers = parser.add_subparsers(dest="command")

    # top
    top_p = subparsers.add_parser("top", help="Топ участников по сообщениям")
    top_p.add_argument("limit", type=int, nargs="?", default=20, help="Количество строк")
    top_p.add_argument("--chat-id", type=str, default=None)
    top_p.add_argument("--days", type=int, default=None, help="Период в днях")
    top_p.add_argument("--json", action="store_true", help="Вывод в формате JSON")

    # stats
    stats_p = subparsers.add_parser("stats", help="Статистика чата")
    stats_p.add_argument("--chat-id", type=str, default=None)
    stats_p.add_argument("--json", action="store_true")

    # sql
    sql_p = subparsers.add_parser("sql", help="Выполнить SQL-запрос")
    sql_p.add_argument("query", type=str)

    # send
    send_p = subparsers.add_parser("send", help="Отправить сообщение в Telegram")
    send_p.add_argument("text", type=str)
    send_p.add_argument("--chat-id", type=str, default=None)
    send_p.add_argument("--reply-to", type=int, default=None)

    # send_photo
    photo_p = subparsers.add_parser("send_photo", help="Отправить фото в Telegram")
    photo_p.add_argument("photo_path", type=str, help="Путь к файлу фото")
    photo_p.add_argument("--caption", type=str, default=None)
    photo_p.add_argument("--chat-id", type=str, default=None)
    photo_p.add_argument("--reply-to", type=int, default=None)

    # search
    search_p = subparsers.add_parser("search", help="Поиск сообщений по тексту и/или пользователю")
    search_p.add_argument("query", type=str, help="Текст для поиска")
    search_p.add_argument("--user", type=str, default=None, help="Имя, юзернейм или ID участника")
    search_p.add_argument("--chat-id", type=str, default=None)
    search_p.add_argument("--limit", type=int, default=25)
    search_p.add_argument("--json", action="store_true")

    # user_history
    uh_p = subparsers.add_parser("user_history", help="История сообщений пользователя")
    uh_p.add_argument("user", type=str, help="Имя, @username или ID участника")
    uh_p.add_argument("--chat-id", type=str, default=None)
    uh_p.add_argument("--limit", type=int, default=30)
    uh_p.add_argument("--json", action="store_true")

    # import_json
    imp_p = subparsers.add_parser("import_json", help="Импорт сообщений из result.json Telegram")
    imp_p.add_argument("path", type=str, help="Путь к файлу result.json")
    imp_p.add_argument("--chat-id", type=str, default="-1004469624876")

    # sync
    sync_p = subparsers.add_parser("sync", help="Синхронизировать переписку через Telethon MTProto")
    sync_p.add_argument("--chat-id", type=int, default=-1004469624876)
    sync_p.add_argument("--limit", type=int, default=None)
    sync_p.add_argument("--full", action="store_true")

    args = parser.parse_args()

    if args.command == "search":
        res = search_messages(args.query, chat_id=args.chat_id, user=args.user, limit=args.limit)
        if args.json:
            print(json.dumps(res, ensure_ascii=False, indent=2))
        else:
            if not res:
                print(f"По запросу '{args.query}' ничего не найдено.")
            else:
                print(f"Найдено {len(res)} сообщений:")
                for r in res:
                    title_hint = f" [{r['custom_title']}]" if r.get('custom_title') else ""
                    un = f" (@{r['username']})" if r.get('username') else ""
                    print(f"[{r.get('dt', '')}] {r.get('full_name')}{title_hint}{un} (msg_id: {r.get('message_id')}): {r.get('text')}")

    elif args.command == "user_history":
        res = get_user_history(args.user, chat_id=args.chat_id, limit=args.limit)
        if args.json:
            print(json.dumps(res, ensure_ascii=False, indent=2))
        else:
            if not res:
                print(f"Сообщений от пользователя {args.user} не найдено.")
            else:
                print(f"Последние сообщения ({len(res)} шт.):")
                for r in res:
                    print(f"[{r.get('dt', '')}] {r.get('full_name')}: {r.get('text')}")

    elif args.command == "import_json":
        cnt = import_export_json(args.path, chat_id=args.chat_id)
        print(f"Успешно импортировано новых сообщений: {cnt}")

    elif args.command == "sync":
        import asyncio
        import sys
        sys.path.insert(0, str(Path(__file__).parent.parent))
        from scripts.sync_chat_history import sync_chat_history
        min_id = None if args.full else None
        cnt = asyncio.run(sync_chat_history(chat_id_int=args.chat_id, limit=args.limit, min_id=min_id))
        print(f"Синхронизировано сообщений: {cnt}")

    elif args.command == "top":
        top = get_top_senders(chat_id=args.chat_id, limit=args.limit, days=args.days)
        if args.json:
            print(json.dumps(top, ensure_ascii=False, indent=2))
        else:
            print(format_top_senders(top, days=args.days))

    elif args.command == "stats":
        stats = get_chat_stats(chat_id=args.chat_id)
        if args.json:
            print(json.dumps(stats, ensure_ascii=False, indent=2))
        else:
            print(f"Статистика сообщений чата:")
            print(f"• Всего сообщений: {stats['total_messages']}")
            print(f"• Уникальных участников: {stats['unique_users']}")
            print(f"• Пиковый день: {stats['peak_date']} ({stats['peak_count']} сообщ.)")

    elif args.command == "sql":
        res = execute_sql(args.query)
        print(json.dumps(res, ensure_ascii=False, indent=2))

    elif args.command == "send":
        resp = send_telegram_message(args.text, chat_id=args.chat_id, reply_to_message_id=args.reply_to)
        print(json.dumps(resp, ensure_ascii=False, indent=2))

    elif args.command == "send_photo":
        resp = send_telegram_photo(args.photo_path, caption=args.caption, chat_id=args.chat_id, reply_to_message_id=args.reply_to)
        print(json.dumps(resp, ensure_ascii=False, indent=2))

    else:
        parser.print_help()

if __name__ == "__main__":
    main()
