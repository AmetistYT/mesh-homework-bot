import html
import json
import os
import re
import sqlite3
import subprocess
import urllib.parse
import requests

def get_base_cookies(db_path):
    conn = sqlite3.connect(db_path)
    c = conn.cursor()
    c.execute("SELECT value FROM settings WHERE key='mesh_cookies_json'")
    row = c.fetchone()
    cookies = json.loads(row[0]) if row else []
    conn.close()
    return cookies

def make_session(cookies):
    s = requests.Session()
    s.headers.update({
        'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36'
    })
    for ck in cookies:
        s.cookies.set(ck['name'], ck['value'], domain=ck.get('domain', '.mos.ru'), path=ck.get('path', '/'))
    return s

def run_test():
    base_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    db_path = os.path.join(base_dir, 'data', 'homeworks.db')

    cookies = get_base_cookies(db_path)
    out = subprocess.check_output(['python3', 'scripts/refresh_mesh_sso.py'], cwd=base_dir)
    token = json.loads(out)['token']

    # Step 1: Diary launch link
    diary_headers = {
        'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36',
        'Authorization': f'Bearer {token}',
        'Auth-Token': token,
        'Profile-Id': '17486681',
        'x-mes-subsystem': 'familymp',
        'client-type': 'diary-mobile',
    }
    r1 = requests.get(
        'https://school.mos.ru/api/ej/partners/v1/homeworks/launch?homework_entry_id=350252328&material_id=f030f422-d47c-5808-8f4b-37c4cb31461f',
        headers=diary_headers,
        allow_redirects=False
    )
    launch_v1_launch = r1.text.strip()
    print('1. Diary launch URL returned:', launch_v1_launch[:80])

    # Step 2: Launcher url
    s_launcher = make_session(cookies)
    launch_v1_url = launch_v1_launch.replace('/v1/launch', '/v1/url')
    r2 = s_launcher.get(launch_v1_url)
    launch_uchebnik = r2.text.strip()
    print('2. Launcher target player URL:', launch_uchebnik[:100])

    parsed = urllib.parse.urlparse(launch_uchebnik)
    params = urllib.parse.parse_qs(parsed.query)

    actor = params.get('actor', [''])[0]
    activity_id = params.get('activityId', [''])[0]
    registration = params.get('registration', [''])[0]
    context = params.get('context', [''])[0]
    print(f'   Extracted actor: {bool(actor)}, activityId: {activity_id}, registration: {registration}')

    # Step 3: OAuth exchange for uchebnik.mos.ru session (fresh session with base cookies)
    s_auth = make_session(cookies)
    auth_url = 'https://school.mos.ru/v1/oauth/authorize?client_id=library&redirect_uri=https%3A%2F%2Fuchebnik.mos.ru%2Flauncher%2Fauthenticate&response_mode=query&response_type=code&scope=email+phone+profile+oic_given_name+oic_family_name+oic_middle_name+oic_birthdate&state=auth-check-flow'
    r_code = s_auth.get(auth_url)
    code_match = re.search(r'code=([a-f0-9-]+)', r_code.text)
    if not code_match:
        print('Failed to extract OAuth code! HTML:', r_code.text[:500])
        return

    code = code_match.group(1)
    print(f'3. Extracted OAuth code: {code}')

    res = s_auth.get(f'https://uchebnik.mos.ru/acl/api/session/v2/callback?code={code}&state=auth-check-flow&rgn=77')
    session_data = res.json()
    eom_token = session_data['accessTokenEom']
    profile_id = session_data['profileId']
    print(f'4. Obtained EOM Token (profileId: {profile_id})')

    # Step 4: Challenge API
    challenge_match = re.search(r'/exam/challenge/(\d+)', activity_id)
    challenge_id = challenge_match.group(1) if challenge_match else '142006653'
    print(f'5. Target Challenge ID: {challenge_id}')

    test_headers = {
        'Authorization': f'Bearer {eom_token}',
        'Profile-Id': str(profile_id),
        'User-Id': '3163720',
        'Profile': json.dumps({'id': int(profile_id), 'type': 'student'}),
        'X-Mes-Subsystem': 'testw',
        'Content-Type': 'application/json',
        'Accept': 'application/json'
    }

    # Fetch challenge info
    info_resp = s_auth.get(f'https://uchebnik.mos.ru/webtests/exam/rest/secure/challenge/{challenge_id}/info', headers=test_headers)
    print('   Challenge info status:', info_resp.status_code)
    if info_resp.status_code == 200:
        info_json = info_resp.json()
        print(f"   Challenge title: {info_json.get('title')}, task count: {info_json.get('task_count')}, max duration: {info_json.get('duration_minutes')}")

    # Now call start-attempt
    body = {
        'actorParam': actor,
        'registrationParam': registration,
        'activityIdParam': activity_id,
        'resultOverviewUrl': f'https://uchebnik.mos.ru/exam/challenge/{challenge_id}/attempt/{{attemptId}}',
        'resultManagementUrl': f'https://uchebnik.mos.ru/exam/challenge/{challenge_id}/context/{{assignmentContextId}}/attempt/{{attemptId}}',
        'context': context,
        'contextCorrupted': False
    }

    r_attempt = s_auth.post(f'https://uchebnik.mos.ru/webtests/exam/rest/secure/challenge/{challenge_id}/start-attempt', headers=test_headers, json=body)
    print('6. Start Attempt Status:', r_attempt.status_code)
    if r_attempt.status_code in (200, 201):
        attempt_data = r_attempt.json()
        print('   Success! Attempt payload keys:', list(attempt_data.keys()))
        attempt_id = attempt_data.get('challenge_attempt_id') or attempt_data.get('id')
        print(f'   Attempt ID: {attempt_id}')
        out_file = os.path.join(base_dir, 'data', 'sample_attempt.json')
        with open(out_file, 'w', encoding='utf-8') as f:
            json.dump(attempt_data, f, ensure_ascii=False, indent=2)
        print(f'   Saved {out_file}')
    else:
        print('   Error response:', r_attempt.text)

if __name__ == '__main__':
    run_test()
