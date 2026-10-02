#!/usr/bin/env python3
import json
import sqlite3
import re
import sys
from pathlib import Path
from datetime import datetime

def extract_text(raw_text):
    if not raw_text:
        return ""
    if isinstance(raw_text, str):
        return raw_text
    if isinstance(raw_text, list):
        res = []
        for item in raw_text:
            if isinstance(item, str):
                res.append(item)
            elif isinstance(item, dict) and "text" in item:
                res.append(str(item["text"]))
        return "".join(res)
    return str(raw_text)

def parse_user_id(from_id_str):
    if not from_id_str:
        return 0
    digits = re.findall(r'\d+', str(from_id_str))
    if digits:
        return int(digits[0])
    return 0

def import_json(json_path: str, db_path: str):
    print(f"Загрузка JSON: {json_path}")
    with open(json_path, "r", encoding="utf-8") as f:
        data = json.load(f)

    raw_chat_id = str(data.get("id", "4469624876"))
    if not raw_chat_id.startswith("-100"):
        chat_id = f"-100{raw_chat_id}"
    else:
        chat_id = raw_chat_id

    messages = data.get("messages", [])
    print(f"Всего сообщений в файле: {len(messages)}, чат: {chat_id}")

    conn = sqlite3.connect(db_path)
    cursor = conn.cursor()

    seen_map = {}
    try:
        cursor.execute("SELECT user_id, username, first_name, last_name, custom_title FROM seen_users")
        for r in cursor.fetchall():
            seen_map[r[0]] = {
                "username": r[1],
                "first_name": r[2],
                "last_name": r[3],
                "custom_title": r[4],
            }
    except Exception as e:
        print(f"Замечание по seen_users: {e}")

    inserted = 0
    updated_users = 0

    cursor.execute("BEGIN TRANSACTION")
    for m in messages:
        msg_id = m.get("id")
        if not msg_id:
            continue

        raw_from_id = m.get("from_id") or m.get("actor_id")
        user_id = parse_user_id(raw_from_id)
        from_name = m.get("from") or m.get("actor") or "Неизвестный"
        
        text = extract_text(m.get("text"))
        if not text.strip():
            if "photo" in m:
                text = "[Фото]"
            elif "sticker_emoji" in m or m.get("media_type") == "sticker":
                text = f"[Стикер {m.get('sticker_emoji', '')}]".strip()
            elif "poll" in m:
                text = f"[Опрос: {m.get('poll', {}).get('question', '')}]"
            elif m.get("media_type") == "voice_message":
                text = "[Голосовое сообщение]"
            elif m.get("media_type") == "video_file":
                text = "[Видео]"
            elif m.get("media_type") == "animation":
                text = "[GIF]"
            elif "file" in m:
                text = f"[Файл: {m.get('file_name', '')}]"
            elif m.get("type") == "service":
                action = m.get("action", "")
                text = f"[Служебное: {action}]"

        reply_to_msg_id = m.get("reply_to_message_id")
        
        raw_date_unix = m.get("date_unixtime")
        if raw_date_unix:
            timestamp = int(raw_date_unix)
        else:
            timestamp = 0

        date_str = m.get("date", "")
        if not date_str:
            date_str = datetime.utcfromtimestamp(timestamp).strftime("%Y-%m-%d %H:%M:%S")

        username = None
        if user_id in seen_map:
            username = seen_map[user_id].get("username")

        cursor.execute(
            """
            INSERT INTO chat_messages (
                chat_id, message_id, user_id, username, full_name, text,
                reply_to_message_id, reply_to_user_id, reply_to_text,
                timestamp, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?, ?)
            ON CONFLICT(chat_id, message_id) DO UPDATE SET
                text = CASE WHEN chat_messages.text = '' OR chat_messages.text LIKE '[%]' THEN excluded.text ELSE chat_messages.text END,
                timestamp = excluded.timestamp
            """,
            (
                chat_id,
                msg_id,
                user_id,
                username,
                from_name,
                text,
                reply_to_msg_id,
                timestamp,
                date_str,
            ),
        )
        if cursor.rowcount > 0:
            inserted += 1

        if user_id > 0 and user_id not in seen_map:
            cursor.execute(
                """
                INSERT OR IGNORE INTO seen_users (user_id, username, first_name, last_name, last_seen, custom_title)
                VALUES (?, ?, ?, '', ?, '')
                """,
                (user_id, None, from_name, date_str),
            )
            seen_map[user_id] = {"username": None, "first_name": from_name, "last_name": "", "custom_title": ""}
            updated_users += 1

    conn.commit()
    print(f"Готово! Обработано записей: {inserted}, новых пользователей: {updated_users}")
    
    cursor.execute("SELECT count(*) FROM chat_messages WHERE chat_id = ?", (chat_id,))
    total_in_db = cursor.fetchone()[0]
    print(f"Всего сообщений в базе данных для чата {chat_id}: {total_in_db}")
    conn.close()

if __name__ == "__main__":
    if len(sys.argv) < 3:
        print("Использование: python3 import_chat_json.py <result.json> <homeworks.db>")
        sys.exit(1)
    import_json(sys.argv[1], sys.argv[2])
