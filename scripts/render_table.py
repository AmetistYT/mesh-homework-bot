#!/usr/bin/env python3
import sys
import os
import json
import textwrap
from PIL import Image, ImageDraw, ImageFont

def render_dark_table(headers, rows, output_path, title=None):
    # Цветовая палитра Dark Mode
    BG_COLOR = (18, 20, 24)        # Фон изображения
    CARD_BG = (26, 29, 36)         # Карточка
    HEADER_BG = (38, 43, 54)       # Шапка таблицы
    ROW_ALT_BG = (31, 35, 44)      # Чередующаяся строка
    BORDER_COLOR = (50, 56, 70)    # Границы
    TEXT_MAIN = (240, 243, 246)    # Основной текст
    ACCENT_COLOR = (88, 166, 255)  # Акцент заголовка

    # Подбор шрифтов с поддержкой кириллицы
    font_paths = [
        '/usr/share/fonts/TTF/DejaVuSans-Bold.ttf',
        '/usr/share/fonts/TTF/DejaVuSans.ttf',
        '/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf',
        '/usr/share/fonts/noto/NotoSans-Bold.ttf'
    ]
    
    font_bold_path = None
    font_regular_path = None
    for p in font_paths:
        if os.path.exists(p):
            if 'Bold' in p and not font_bold_path:
                font_bold_path = p
            elif not font_regular_path:
                font_regular_path = p

    try:
        font_title = ImageFont.truetype(font_bold_path or font_paths[0], 24)
        font_header = ImageFont.truetype(font_bold_path or font_paths[0], 18)
        font_cell = ImageFont.truetype(font_regular_path or font_paths[1], 16)
    except:
        font_title = font_header = font_cell = ImageFont.load_default()

    col_count = len(headers)
    if col_count == 0:
        return

    padding_x = 20
    padding_y = 14
    margin = 30
    total_w = 960
    col_w = (total_w - margin * 2) // col_count

    # Расчет высоты строк с учетом переноса текста
    wrapped_rows = []
    row_heights = []

    for row in rows:
        max_lines = 1
        wrapped_row = []
        for cell in row:
            chars_per_line = max(15, (col_w - padding_x * 2) // 10)
            wrapped_text = textwrap.fill(str(cell).strip(), width=chars_per_line)
            lines = wrapped_text.split('\n')
            if len(lines) > max_lines:
                max_lines = len(lines)
            wrapped_row.append(lines)
        wrapped_rows.append(wrapped_row)
        row_heights.append(max_lines * 22 + padding_y * 2)

    header_h = 48
    title_h = 56 if title else 0
    total_h = margin * 2 + title_h + header_h + sum(row_heights)

    img = Image.new('RGB', (total_w, total_h), BG_COLOR)
    draw = ImageDraw.Draw(img)

    # Карточка с закругленными углами
    card_box = [margin, margin, total_w - margin, total_h - margin]
    draw.rounded_rectangle(card_box, radius=14, fill=CARD_BG, outline=BORDER_COLOR, width=2)

    cur_y = margin
    if title:
        draw.text((margin + 20, cur_y + 14), title, font=font_title, fill=ACCENT_COLOR)
        cur_y += title_h

    # Шапка таблицы
    draw.rectangle([margin, cur_y, total_w - margin, cur_y + header_h], fill=HEADER_BG)
    for c_idx, h in enumerate(headers):
        x = margin + c_idx * col_w + padding_x
        draw.text((x, cur_y + 14), h.strip(), font=font_header, fill=TEXT_MAIN)
    cur_y += header_h
    draw.line([margin, cur_y, total_w - margin, cur_y], fill=BORDER_COLOR, width=2)

    # Строки таблицы
    for r_idx, (wrapped_row, r_h) in enumerate(zip(wrapped_rows, row_heights)):
        row_bg = ROW_ALT_BG if r_idx % 2 == 1 else CARD_BG
        draw.rectangle([margin, cur_y, total_w - margin, cur_y + r_h], fill=row_bg)

        for c_idx, lines in enumerate(wrapped_row):
            x = margin + c_idx * col_w + padding_x
            y = cur_y + padding_y
            for line in lines:
                draw.text((x, y), line, font=font_cell, fill=TEXT_MAIN)
                y += 22

        cur_y += r_h
        draw.line([margin, cur_y, total_w - margin, cur_y], fill=BORDER_COLOR, width=1)

    os.makedirs(os.path.dirname(os.path.abspath(output_path)), exist_ok=True)
    img.save(output_path)
    print(f"Table rendered to {output_path}")

def parse_markdown_table(text):
    lines = [l.strip() for l in text.split('\n') if l.strip().startswith('|')]
    if len(lines) < 2:
        return None, None

    # Заголовки
    header_line = lines[0]
    headers = [c.strip() for c in header_line.split('|')[1:-1]]

    # Пропуск разделителя |---|---|
    data_lines = lines[1:]
    if data_lines and all(c in '-:| ' for c in data_lines[0].replace('|', '')):
        data_lines = data_lines[1:]

    rows = []
    for dl in data_lines:
        cells = [c.strip() for c in dl.split('|')[1:-1]]
        if len(cells) < len(headers):
            cells.extend([''] * (len(headers) - len(cells)))
        rows.append(cells[:len(headers)])

    return headers, rows

if __name__ == '__main__':
    if len(sys.argv) < 3:
        print("Usage: render_table.py <input_md_file_or_text> <output_png_path> [title]")
        sys.exit(1)

    input_arg = sys.argv[1]
    output_png = sys.argv[2]
    title = sys.argv[3] if len(sys.argv) > 3 else None

    if os.path.exists(input_arg):
        with open(input_arg, 'r', encoding='utf-8') as f:
            content = f.read()
    else:
        content = input_arg

    headers, rows = parse_markdown_table(content)
    if headers and rows:
        render_dark_table(headers, rows, output_png, title)
    else:
        print("No valid markdown table found")
        sys.exit(2)
