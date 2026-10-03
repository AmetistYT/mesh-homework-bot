import json
import subprocess
import requests

import os

def get_token():
    base_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    out = subprocess.check_output(["python3", "scripts/refresh_mesh_sso.py"], cwd=base_dir)
    data = json.loads(out)
    return data["token"]

token = get_token()
student_id = "17486681"

headers = {
    "Authorization": f"Bearer {token}",
    "Auth-Token": token,
    "Profile-Id": student_id,
    "x-mes-subsystem": "familymp",
    "client-type": "diary-mobile",
    "User-Agent": "okhttp/4.9.3",
    "Accept": "application/json"
}

# 1. Fetch homeworks for current week
url = "https://school.mos.ru/api/family/mobile/v1/homeworks/short"
params = {
    "student_id": student_id,
    "from": "2026-09-28",
    "to": "2026-10-10",
    "sort_column": "date",
    "sort_direction": "asc"
}
resp = requests.get(url, headers=headers, params=params)
print("Homeworks status:", resp.status_code)
hw_data = resp.json().get("payload", [])
print(f"Total homeworks: {len(hw_data)}")

cdz_entries = []
for hw in hw_data:
    subj = hw.get("subject_name", "")
    desc = hw.get("description", "")
    hw_id = hw.get("homework_entry_student_id")
    attachments = hw.get("attachments", [])
    has_test = any(att.get("link") or "uchebnik" in str(att) for att in attachments) or "цдз" in desc.lower() or "тест" in desc.lower()
    if has_test or attachments:
        cdz_entries.append(hw)
        print(f"HW #{hw_id} | {subj} | {desc[:40]} | Attachments: {attachments}")

# Also check additional_materials endpoint
add_url = "https://school.mos.ru/api/family/materials/v1/additional_materials"
add_resp = requests.post(add_url, headers=headers, json={"student_id": int(student_id), "from": "2026-09-28", "to": "2026-10-10"})
print("Additional materials status:", add_resp.status_code)
if add_resp.status_code == 200:
    print("Additional materials sample:", json.dumps(add_resp.json(), ensure_ascii=False, indent=2)[:1000])
