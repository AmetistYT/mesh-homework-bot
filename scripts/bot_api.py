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

    # sync
    sync_p = subparsers.add_parser("sync", help="Синхронизировать переписку через Telethon MTProto")
    sync_p.add_argument("--chat-id", type=int, default=-1004469624876)
    sync_p.add_argument("--limit", type=int, default=None)
    sync_p.add_argument("--full", action="store_true")

    args = parser.parse_args()

    if args.command == "sync":
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
