#!/usr/bin/env python3
"""
Генерация визуализаций:
1. График рейтинга участников по сообщениям с ненормативной лексикой (1 сообщение = 1 балл)
2. Облако слов (все повторения слов с частотой от 6 упоминаний)
"""

import os
import sys
import re
import sqlite3
import argparse
from collections import Counter
from pathlib import Path
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from wordcloud import WordCloud

# Добавляем родительский каталог в sys.path для импорта bot_api
sys.path.insert(0, str(Path(__file__).parent.parent))
from scripts.bot_api import send_telegram_photo, get_default_chat_id

SWEAR_PATTERNS = [
    r'\bху[йяеёию]|хули|поху[йи]|наху[йи]|доху[яи]|ниху[яи]',
    r'\bпизд',
    r'\b[её]б[аеёиу]|ебл|ёбл|выеб|заеб|наеб|проеб|уеб|ебан|ебат',
    r'\bбл[яя][тть]*\b|\bбля\b|\bблять\b|\bлять\b',
    r'\bмуд[аоеи]',
    r'\bсук[аиое]?\b|\bсуч[кч]',
    r'\bгондон|\bгандон',
    r'\bпид[оа]р|\bпидр',
    r'\bшлюх',
    r'\bебло|\bеблет',
    r'\bдолбо[её]б',
    r'\bсос[иа]ть|\bсоси\b',
    r'\bзаткнись|\bзавали',
    r'\bчмо\b|\bдаун\b|\bдебил\b|\bурод\b'
]
combined_pattern = re.compile('|'.join(SWEAR_PATTERNS), re.IGNORECASE)

def clean_for_plot(name: str) -> str:
    res = []
    for ch in name:
        if ch.isalnum() or ch in " ()_@-.,!":
            res.append(ch)
    cleaned = "".join(res).strip()
    return cleaned if cleaned else "Пользователь"

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

def analyze_chat(chat_id: str = None, send_to_tg: bool = False, reply_to: int = None):
    db_path = find_db_path()
    conn = sqlite3.connect(db_path)
    cursor = conn.cursor()

    users = {}
    try:
        cursor.execute("SELECT user_id, username, first_name, last_name, custom_title FROM seen_users")
        for uid, uname, fn, ln, title in cursor.fetchall():
            users[uid] = {
                "username": uname,
                "first_name": fn,
                "last_name": ln,
                "title": title or "",
            }
    except Exception as e:
        print(f"Ошибка загрузки seen_users: {e}")

    cursor.execute("SELECT message_id, user_id, full_name, username, text FROM chat_messages WHERE text IS NOT NULL AND text != ''")
    rows = cursor.fetchall()
    conn.close()

    user_msgs_count = Counter()
    user_names = {}
    plot_names = {}
    word_freq = Counter()

    # Известные красивые имена
    custom_name_map = {
        6731822991: "Алина (@yeo3ange)",
        8026023851: "Имран",
        8193800816: "Семён (@Ametist1337)",
        8469803740: "Асыл (@ahzkto67)",
        1665877840: "Василя",
        7466301796: "Андрей (@trlcly)",
        7838916464: "Ассоль",
        8566127992: "Тимофей / Жыртракс (@zxcdota2kitty)",
        8732018819: "Лиза (@bwrmftid)",
        5393284763: "Варя (@gussiavaffe)",
    }

    for mid, uid, fname, uname, text in rows:
        if uid == 0 or uid == 8127794104:
            continue

        words = re.findall(r'[а-яёa-z0-9_-]+', text.lower())
        found_in_msg = []
        for w in words:
            if combined_pattern.search(w):
                found_in_msg.append(w)
                word_freq[w] += 1

        if found_in_msg:
            user_msgs_count[uid] += 1
            if uid not in user_names:
                if uid in custom_name_map:
                    user_names[uid] = custom_name_map[uid]
                    plot_names[uid] = clean_for_plot(custom_name_map[uid])
                else:
                    u_info = users.get(uid, {})
                    title = u_info.get("title", "")
                    if title:
                        display = f"{title.capitalize()} ({fname})"
                    elif u_info.get("username"):
                        display = f"{fname} (@{u_info['username']})"
                    else:
                        display = fname
                    user_names[uid] = display
                    plot_names[uid] = clean_for_plot(display)

    # 1. Построение графика рейтинга участников
    top_users = user_msgs_count.most_common(8)
    top_users.reverse()

    names_for_plot = [plot_names.get(uid, f"User {uid}") for uid, _ in top_users]
    counts = [c for _, c in top_users]

    plt.figure(figsize=(10, 5.5), facecolor="#18181b")
    ax = plt.subplot(111)
    ax.set_facecolor("#18181b")

    bars = ax.barh(names_for_plot, counts, color="#e11d48", height=0.62, edgecolor="#fb7185", linewidth=1.2)

    for bar in bars:
        width = bar.get_width()
        ax.text(width + 4, bar.get_y() + bar.get_height()/2, f"{int(width)}",
                va="center", ha="left", color="#ffffff", fontsize=11, fontweight="bold")

    ax.set_title("Рейтинг участников по сообщениям с матом и агрессией", color="#ffffff", fontsize=13, pad=15, fontweight="bold")
    ax.set_xlabel("Количество сообщений с ненормативной лексикой (1 сообщ. = 1 балл)", color="#a1a1aa", fontsize=10, labelpad=10)
    ax.tick_params(colors="#f4f4f5", labelsize=10.5)
    ax.spines["top"].set_visible(False)
    ax.spines["right"].set_visible(False)
    ax.spines["left"].set_color("#3f3f46")
    ax.spines["bottom"].set_color("#3f3f46")
    ax.grid(axis="x", linestyle="--", alpha=0.15, color="#ffffff")
    max_c = max(counts) if counts else 100
    ax.set_xlim(0, max_c + 35)

    plt.tight_layout()
    chart_path = "/tmp/swear_ranking.png"
    plt.savefig(chart_path, dpi=200, facecolor="#18181b")
    plt.close()
    print(f"График сохранен в {chart_path}")

    # 2. Построение облака слов (слова с частотой >= 6)
    filtered_words = {w: c for w, c in word_freq.items() if c >= 6}
    print(f"Слов с частотой >= 6: {len(filtered_words)}")

    wc = WordCloud(
        width=1200,
        height=800,
        background_color="#18181b",
        colormap="Reds",
        max_words=100,
        contour_width=0,
        collocations=False
    ).generate_from_frequencies(filtered_words)

    plt.figure(figsize=(12, 8), facecolor="#18181b")
    plt.imshow(wc, interpolation="bilinear")
    plt.axis("off")
    plt.tight_layout(pad=0)

    cloud_path = "/tmp/swear_wordcloud.png"
    plt.savefig(cloud_path, dpi=200, facecolor="#18181b")
    plt.close()
    print(f"Облако слов сохранено в {cloud_path}")

    # Текстовые отчеты
    chart_caption = "*Рейтинг участников по сообщениям с матом:*\n\n"
    rank = 1
    for uid, count in user_msgs_count.most_common(8):
        name = user_names.get(uid, f"User {uid}")
        chart_caption += f"{rank}. *{name}* — `{count}` сообщ.\n"
        rank += 1
    chart_caption += "\n(Правило: 1 сообщение с матом = 1 балл в рейтинге)"

    cloud_caption = "*Облако популярной ненормативной лексики:*\n\n"
    cloud_caption += "Слова с частотой от 6 упоминаний за всю историю (9 900+ сообщений):\n"
    top_words = sorted(filtered_words.items(), key=lambda x: x[1], reverse=True)
    for w, c in top_words:
        cloud_caption += f"• `{w}` — {c}\n"

    if send_to_tg:
        target_chat = chat_id or get_default_chat_id()
        print(f"Отправка графика в чат {target_chat}...")
        send_telegram_photo(chart_path, caption=chart_caption, chat_id=target_chat, reply_to_message_id=reply_to)
        print(f"Отправка облака слов в чат {target_chat}...")
        send_telegram_photo(cloud_path, caption=cloud_caption, chat_id=target_chat, reply_to_message_id=reply_to)
        print("Визуализации успешно отправлены в Telegram!")

    return chart_path, cloud_path, chart_caption, cloud_caption

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Анализ мата и генерация визуализаций")
    parser.add_argument("--send", action="store_true", help="Отправить результаты в Telegram")
    parser.add_argument("--chat-id", type=str, default=None, help="ID чата Telegram")
    parser.add_argument("--reply-to", type=int, default=None, help="ID сообщения для ответа")
    args = parser.parse_args()

    analyze_chat(chat_id=args.chat_id, send_to_tg=args.send, reply_to=args.reply_to)
