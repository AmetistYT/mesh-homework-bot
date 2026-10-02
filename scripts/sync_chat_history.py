#!/usr/bin/env python3
"""
Скрипт синхронизации истории переписки Telegram supergroup через MTProto Telethon.
Позволяет скачивать пропущенные или всю историю сообщений напрямую в SQLite базу данных (chat_messages).
"""

import os
import sys
import asyncio
import sqlite3
import argparse
from pathlib import Path
from datetime import datetime

API_ID = 2040
API_HASH = "b18441a1ff607e10a989891a5462e627"

def find_session_path() -> str:
    candidates = [
        Path("/root/projects/mesh-homework-bot/data/my_session.session"),
        Path("/home/ametist1337/projects/mesh-homework-bot/data/my_session.session"),
        Path("/home/ametist1337/projects/tg_quick_bot/my_session.session"),
        Path("data/my_session.session"),
    ]
    for c in candidates:
        if c.exists():
            return str(c).replace(".session", "")
    return "data/my_session"

def find_db_path() -> str:
    candidates = [
        Path("/root/projects/mesh-homework-bot/data/homeworks.db"),
        Path("/home/ametist1337/projects/mesh-homework-bot/data/homeworks.db"),
        Path("data/homeworks.db"),
    ]
    for c in candidates:
        if c.exists():
            return str(c)
    return "data/homeworks.db"

async def sync_chat_history(chat_id_int: int = -1004469624876, limit: int = None, min_id: int = None, max_id: int = None):
    try:
        from telethon import TelegramClient
        from telethon.tl.types import User, Channel, Chat
    except ImportError:
        print("Ошибка: telethon не установлен. Установите: pip install telethon")
        return 0

    session_path = find_session_path()
    db_path = find_db_path()
    chat_id_str = str(chat_id_int)

    conn = sqlite3.connect(db_path)
    cursor = conn.cursor()

    # Если min_id не задан, синхронизируем новые сообщения с момента последнего в базе
    if min_id is None and max_id is None:
        cursor.execute("SELECT max(message_id) FROM chat_messages WHERE chat_id = ?", (chat_id_str,))
        row = cursor.fetchone()
        if row and row[0]:
            min_id = row[0]
            print(f"Последнее сообщение в базе: #{min_id}. Синхронизируем свежие сообщения...")

    client = TelegramClient(session_path, API_ID, API_HASH)
    await client.connect()

    if not await client.is_user_authorized():
        print("Ошибка: клиент Telethon не авторизован.")
        await client.disconnect()
        conn.close()
        return 0

    try:
        entity = await client.get_entity(chat_id_int)
    except Exception as e:
        print(f"Не удалось получить чат {chat_id_int}: {e}")
        await client.disconnect()
        conn.close()
        return 0

    print(f"Подключено к чату: {getattr(entity, 'title', entity.id)}")

    count = 0
    users_cache = {}

    cursor.execute("BEGIN TRANSACTION")

    # Итерируемся по сообщениям
    iter_args = {"reverse": True} if min_id else {}
    if min_id:
        iter_args["min_id"] = min_id
    if max_id:
        iter_args["max_id"] = max_id
    if limit:
        iter_args["limit"] = limit

    async for msg in client.iter_messages(entity, **iter_args):
        mid = msg.id
        uid = msg.sender_id or 0
        text = msg.text or ""

        if not text.strip():
            if msg.photo:
                text = "[Фото]"
            elif msg.sticker:
                emoji = getattr(msg.sticker, "attributes", [])
                text = "[Стикер]"
            elif msg.poll:
                text = f"[Опрос: {getattr(msg.poll.poll, 'question', '')}]"
            elif msg.voice:
                text = "[Голосовое сообщение]"
            elif msg.video:
                text = "[Видео]"
            elif msg.document:
                text = f"[Файл: {getattr(msg.file, 'name', '')}]"
            elif msg.action:
                text = f"[Служебное: {type(msg.action).__name__}]"

        reply_to_mid = msg.reply_to_msg_id

        date_dt = msg.date
        ts = int(date_dt.timestamp())
        created_at_str = date_dt.strftime("%Y-%m-%d %H:%M:%S")

        # Получаем данные отправителя
        first_name = ""
        last_name = ""
        username = None
        if uid not in users_cache:
            sender = await msg.get_sender()
            if isinstance(sender, User):
                first_name = sender.first_name or ""
                last_name = sender.last_name or ""
                username = sender.username
            elif isinstance(sender, (Channel, Chat)):
                first_name = getattr(sender, "title", "Группа")
            users_cache[uid] = (first_name, last_name, username)
        else:
            first_name, last_name, username = users_cache[uid]

        full_name = f"{first_name} {last_name}".strip()
        if not full_name:
            full_name = "Пользователь"

        cursor.execute(
            """
            INSERT INTO chat_messages (
                chat_id, message_id, user_id, username, full_name, text,
                reply_to_message_id, reply_to_user_id, reply_to_text,
                timestamp, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?, ?)
            ON CONFLICT(chat_id, message_id) DO UPDATE SET
                text = excluded.text,
                timestamp = excluded.timestamp
            """,
            (
                chat_id_str,
                mid,
                uid,
                username,
                full_name,
                text,
                reply_to_mid,
                ts,
                created_at_str,
            ),
        )

        if uid > 0:
            cursor.execute(
                """
                INSERT INTO seen_users (user_id, username, first_name, last_name, last_seen, custom_title)
                VALUES (?, ?, ?, ?, ?, '')
                ON CONFLICT(user_id) DO UPDATE SET
                    username = COALESCE(excluded.username, seen_users.username),
                    first_name = COALESCE(excluded.first_name, seen_users.first_name),
                    last_name = COALESCE(excluded.last_name, seen_users.last_name),
                    last_seen = excluded.last_seen
                """,
                (uid, username, first_name, last_name, created_at_str),
            )

        count += 1
        if count % 200 == 0:
            conn.commit()
            cursor.execute("BEGIN TRANSACTION")
            print(f"Синхронизировано {count} сообщений...")

    conn.commit()
    await client.disconnect()
    conn.close()

    print(f"Синхронизация завершена. Добавлено/обновлено {count} сообщений в базе данных.")
    return count

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Синхронизация переписки с чатом Telegram")
    parser.add_argument("--chat-id", type=int, default=-1004469624876, help="ID супергруппы Telegram")
    parser.add_argument("--limit", type=int, default=None, help="Ограничение количества сообщений")
    parser.add_argument("--min-id", type=int, default=None, help="Синхронизировать начиная с message_id")
    parser.add_argument("--full", action="store_true", help="Синхронизировать всю доступную историю с нуля")

    args = parser.parse_args()
    min_id = None if args.full else args.min_id
    asyncio.run(sync_chat_history(chat_id_int=args.chat_id, limit=args.limit, min_id=min_id))
