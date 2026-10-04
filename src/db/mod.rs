use anyhow::{Context, Result};
use chrono::Local;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ProcessedHomeworkRecord {
    pub id: i64,
    pub homework_entry_student_id: i64,
    pub subject_name: String,
    pub lesson_date: String,
    pub description: String,
    pub solution: Option<String>,
    pub status: String,
    pub telegram_message_id: Option<i64>,
    pub is_pinned: bool,
    pub pinned_at: Option<String>,
    pub unpinned_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct DbStats {
    pub total: i64,
    pub sent: i64,
    pub failed: i64,
    pub pending: i64,
}

#[derive(Debug, Clone)]
pub struct PermissionBackupRecord {
    pub id: i64,
    pub chat_id: String,
    pub user_id: i64,
    pub username: Option<String>,
    pub original_status: String,
    pub custom_title: Option<String>,
    pub admin_rights_json: Option<String>,
    pub permissions_json: Option<String>,
    pub restore_at: Option<i64>,
    pub restored: bool,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct ChatMessageRecord {
    pub id: i64,
    pub chat_id: String,
    pub message_id: i64,
    pub user_id: i64,
    pub username: Option<String>,
    pub full_name: String,
    pub text: String,
    pub reply_to_message_id: Option<i64>,
    pub reply_to_user_id: Option<i64>,
    pub reply_to_text: Option<String>,
    pub timestamp: i64,
    pub created_at: String,
}

#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS processed_homeworks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                homework_entry_student_id INTEGER UNIQUE NOT NULL,
                subject_name TEXT NOT NULL,
                lesson_date TEXT NOT NULL,
                description TEXT,
                solution TEXT,
                status TEXT NOT NULL,
                telegram_message_id INTEGER,
                is_pinned INTEGER DEFAULT 0,
                pinned_at TEXT,
                unpinned_at TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS seen_users (
                user_id INTEGER PRIMARY KEY,
                username TEXT,
                first_name TEXT,
                last_name TEXT,
                last_seen TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS permission_backups (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id TEXT NOT NULL,
                user_id INTEGER NOT NULL,
                username TEXT,
                original_status TEXT NOT NULL,
                custom_title TEXT,
                admin_rights_json TEXT,
                permissions_json TEXT,
                restore_at INTEGER,
                restored INTEGER DEFAULT 0,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chat_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id TEXT NOT NULL,
                message_id INTEGER NOT NULL,
                user_id INTEGER NOT NULL,
                username TEXT,
                full_name TEXT NOT NULL,
                text TEXT NOT NULL,
                reply_to_message_id INTEGER,
                reply_to_user_id INTEGER,
                reply_to_text TEXT,
                timestamp INTEGER NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_hw_entry_id ON processed_homeworks (homework_entry_student_id);
            CREATE INDEX IF NOT EXISTS idx_hw_status ON processed_homeworks (status);
            CREATE INDEX IF NOT EXISTS idx_seen_username ON seen_users (username);
            CREATE INDEX IF NOT EXISTS idx_perm_user ON permission_backups (chat_id, user_id);
            CREATE INDEX IF NOT EXISTS idx_perm_restore ON permission_backups (restore_at, restored);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_chat_msg ON chat_messages (chat_id, message_id);
            CREATE INDEX IF NOT EXISTS idx_chat_time ON chat_messages (chat_id, timestamp);
            ",
        )
        .context("Ошибка создания таблиц SQLite")?;

        // Безопасное добавление новых колонок, если база уже существовала
        let _ = conn.execute("ALTER TABLE processed_homeworks ADD COLUMN is_pinned INTEGER DEFAULT 0", []);
        let _ = conn.execute("ALTER TABLE processed_homeworks ADD COLUMN pinned_at TEXT", []);
        let _ = conn.execute("ALTER TABLE processed_homeworks ADD COLUMN unpinned_at TEXT", []);
        let _ = conn.execute("ALTER TABLE seen_users ADD COLUMN custom_title TEXT", []);

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn is_homework_processed(&self, homework_id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT 1 FROM processed_homeworks WHERE homework_entry_student_id = ?1 LIMIT 1",
        )?;
        let exists = stmt.exists(params![homework_id])?;
        Ok(exists)
    }

    pub fn clear_cache_for_date(&self, date_str: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count = conn.execute(
            "DELETE FROM processed_homeworks WHERE lesson_date = ?1",
            params![date_str],
        )?;
        Ok(count)
    }

    pub fn get_homework_solution(&self, homework_id: i64) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT solution FROM processed_homeworks WHERE homework_entry_student_id = ?1 AND status = 'sent' AND solution IS NOT NULL LIMIT 1",
        )?;
        let mut rows = stmt.query(params![homework_id])?;
        if let Some(row) = rows.next()? {
            let sol: String = row.get(0)?;
            if !sol.trim().is_empty() {
                return Ok(Some(sol));
            }
        }
        Ok(None)
    }

    pub fn insert_pending_homework(
        &self,
        homework_id: i64,
        subject: &str,
        date: &str,
        description: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Local::now().to_rfc3339();

        conn.execute(
            "INSERT OR IGNORE INTO processed_homeworks 
            (homework_entry_student_id, subject_name, lesson_date, description, status, created_at, updated_at) 
            VALUES (?1, ?2, ?3, ?4, 'pending', ?5, ?5)",
            params![homework_id, subject, date, description, now],
        )?;

        Ok(())
    }

    pub fn update_solution(
        &self,
        homework_id: i64,
        solution: &str,
        status: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Local::now().to_rfc3339();

        conn.execute(
            "UPDATE processed_homeworks 
             SET solution = ?1, status = ?2, updated_at = ?3 
             WHERE homework_entry_student_id = ?4",
            params![solution, status, now, homework_id],
        )?;

        Ok(())
    }

    pub fn mark_sent_and_pinned(
        &self,
        homework_id: i64,
        telegram_message_id: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Local::now().to_rfc3339();

        conn.execute(
            "UPDATE processed_homeworks 
             SET status = 'sent', telegram_message_id = ?1, is_pinned = 1, pinned_at = ?2, updated_at = ?2 
             WHERE homework_entry_student_id = ?3",
            params![telegram_message_id, now, homework_id],
        )?;

        Ok(())
    }

    pub fn get_expired_pins(&self, hours_ttl: i64) -> Result<Vec<(i64, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT homework_entry_student_id, telegram_message_id, pinned_at 
             FROM processed_homeworks 
             WHERE is_pinned = 1 AND telegram_message_id IS NOT NULL AND pinned_at IS NOT NULL",
        )?;

        let rows = stmt.query_map([], |row| {
            let hw_id: i64 = row.get(0)?;
            let msg_id: i64 = row.get(1)?;
            let pinned_str: String = row.get(2)?;
            Ok((hw_id, msg_id, pinned_str))
        })?;

        let mut expired = Vec::new();
        let now = Local::now();
        let ttl_duration = chrono::Duration::hours(hours_ttl);

        for r in rows.flatten() {
            let (hw_id, msg_id, pinned_str) = r;
            if let Ok(pinned_dt) = chrono::DateTime::parse_from_rfc3339(&pinned_str) {
                if now.signed_duration_since(pinned_dt) >= ttl_duration {
                    expired.push((hw_id, msg_id));
                }
            }
        }

        Ok(expired)
    }

    pub fn mark_unpinned(&self, homework_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Local::now().to_rfc3339();

        conn.execute(
            "UPDATE processed_homeworks 
             SET is_pinned = 0, unpinned_at = ?1, updated_at = ?1 
             WHERE homework_entry_student_id = ?2",
            params![now, homework_id],
        )?;

        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
        let mut rows = stmt.query(params![key])?;

        if let Some(row) = rows.next()? {
            let val: String = row.get(0)?;
            Ok(Some(val))
        } else {
            Ok(None)
        }
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Local::now().to_rfc3339();

        conn.execute(
            "INSERT INTO settings (key, value, updated_at) 
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, now],
        )?;

        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        Ok(())
    }

    pub fn get_recent_solutions(&self, limit: usize) -> Result<Vec<(String, String, String, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT subject_name, lesson_date, description, solution 
             FROM processed_homeworks 
             WHERE status = 'sent' AND solution IS NOT NULL AND TRIM(solution) != ''
             ORDER BY lesson_date DESC, id DESC 
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            ))
        })?;

        let mut res = Vec::new();
        for r in rows.flatten() {
            res.push(r);
        }
        Ok(res)
    }

    pub fn get_stats(&self) -> Result<DbStats> {
        let conn = self.conn.lock().unwrap();
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM processed_homeworks", [], |r| r.get(0))?;
        let sent: i64 = conn.query_row("SELECT COUNT(*) FROM processed_homeworks WHERE status = 'sent'", [], |r| r.get(0))?;
        let failed: i64 = conn.query_row("SELECT COUNT(*) FROM processed_homeworks WHERE status = 'failed'", [], |r| r.get(0))?;
        let pending: i64 = conn.query_row("SELECT COUNT(*) FROM processed_homeworks WHERE status IN ('pending', 'solving')", [], |r| r.get(0))?;

        Ok(DbStats {
            total,
            sent,
            failed,
            pending,
        })
    }

    pub fn save_seen_user(
        &self,
        user_id: i64,
        username: Option<&str>,
        first_name: &str,
        last_name: Option<&str>,
        custom_title: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Local::now().to_rfc3339();
        let clean_username = username.map(|u| u.trim_start_matches('@').to_lowercase());
        let clean_title = custom_title.map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty());

        conn.execute(
            "INSERT INTO seen_users (user_id, username, first_name, last_name, custom_title, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(user_id) DO UPDATE SET
                username = COALESCE(excluded.username, seen_users.username),
                first_name = excluded.first_name,
                last_name = COALESCE(excluded.last_name, seen_users.last_name),
                custom_title = COALESCE(excluded.custom_title, seen_users.custom_title),
                last_seen = excluded.last_seen",
            params![user_id, clean_username, first_name, last_name, clean_title, now],
        )?;
        Ok(())
    }

    pub fn find_user_by_username(&self, username: &str) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        let clean = username.trim_start_matches('@').to_lowercase();
        let mut stmt = conn.prepare("SELECT user_id FROM seen_users WHERE lower(username) = ?1 LIMIT 1")?;
        let mut rows = stmt.query(params![clean])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn find_user_by_name_or_username(&self, query: &str) -> Result<Option<(i64, Option<String>)>> {
        let conn = self.conn.lock().unwrap();
        let clean = query.trim_start_matches('@').trim().to_lowercase();
        if clean.is_empty() {
            return Ok(None);
        }

        let mut stmt = conn.prepare("SELECT user_id, username, first_name, last_name, custom_title FROM seen_users")?;
        let rows = stmt.query_map([], |row| {
            let uid: i64 = row.get(0)?;
            let un: Option<String> = row.get(1)?;
            let fn_: Option<String> = row.get(2)?;
            let ln: Option<String> = row.get(3)?;
            let ct: Option<String> = row.get(4).ok().flatten();
            Ok((uid, un, fn_, ln, ct))
        })?;

        let mut exact_match = None;
        let mut partial_match = None;

        for r in rows.flatten() {
            let (uid, un, fn_, ln, ct) = r;
            let un_low = un.as_deref().unwrap_or("").to_lowercase();
            let fn_low = fn_.as_deref().unwrap_or("").to_lowercase();
            let ln_low = ln.as_deref().unwrap_or("").to_lowercase();
            let ct_low = ct.as_deref().unwrap_or("").to_lowercase();

            if (!un_low.is_empty() && un_low == clean)
                || (!fn_low.is_empty() && fn_low == clean)
                || (!ln_low.is_empty() && ln_low == clean)
                || (!ct_low.is_empty() && ct_low == clean)
            {
                exact_match = Some((uid, un));
                break;
            }

            if (!ct_low.is_empty() && (ct_low.contains(&clean) || clean.contains(&ct_low)))
                || (!fn_low.is_empty() && (fn_low.contains(&clean) || clean.contains(&fn_low)))
                || (!ln_low.is_empty() && (ln_low.contains(&clean) || clean.contains(&ln_low)))
                || (!un_low.is_empty() && un_low.contains(&clean))
            {
                if partial_match.is_none() {
                    partial_match = Some((uid, un));
                }
            }
        }

        Ok(exact_match.or(partial_match))
    }

    pub fn get_user_titles_map(&self) -> Result<std::collections::HashMap<i64, String>> {
        let conn = self.conn.lock().unwrap();
        let mut map = std::collections::HashMap::new();
        let mut stmt = conn.prepare(
            "SELECT user_id, custom_title FROM seen_users WHERE custom_title IS NOT NULL AND trim(custom_title) != ''"
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        for r in rows.flatten() {
            let (uid, title): (i64, String) = r;
            map.insert(uid, title);
        }
        Ok(map)
    }

    pub fn get_all_known_users_in_chat(&self, chat_id: &str) -> Result<Vec<(i64, Option<String>, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut list = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // 1. Из истории сообщений данного чата
        if let Ok(mut stmt) = conn.prepare(
            "SELECT DISTINCT user_id, username, full_name 
             FROM chat_messages 
             WHERE chat_id = ?1 AND user_id > 0
             ORDER BY id DESC"
        ) {
            if let Ok(rows) = stmt.query_map(params![chat_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            }) {
                for r in rows.flatten() {
                    let (uid, un, fn_): (i64, Option<String>, String) = r;
                    if seen.insert(uid) {
                        list.push((uid, un, fn_));
                    }
                }
            }
        }

        // 2. Из таблицы seen_users
        if let Ok(mut stmt2) = conn.prepare("SELECT user_id, username, first_name, last_name FROM seen_users WHERE user_id > 0") {
            if let Ok(rows2) = stmt2.query_map([], |row| {
                let uid: i64 = row.get(0)?;
                let un: Option<String> = row.get(1)?;
                let fn_: String = row.get(2)?;
                let ln: Option<String> = row.get(3)?;
                let full = match ln {
                    Some(l) if !l.trim().is_empty() => format!("{} {}", fn_, l),
                    _ => fn_,
                };
                Ok((uid, un, full))
            }) {
                for r in rows2.flatten() {
                    let (uid, un, fn_) = r;
                    if seen.insert(uid) {
                        list.push((uid, un, fn_));
                    }
                }
            }
        }

        Ok(list)
    }

    pub fn save_permission_backup(
        &self,
        chat_id: &str,
        user_id: i64,
        username: Option<&str>,
        original_status: &str,
        custom_title: Option<&str>,
        admin_rights_json: Option<&str>,
        permissions_json: Option<&str>,
        restore_at: Option<i64>,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let now = Local::now().to_rfc3339();

        conn.execute(
            "INSERT INTO permission_backups (
                chat_id, user_id, username, original_status, custom_title,
                admin_rights_json, permissions_json, restore_at, restored, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9)",
            params![
                chat_id,
                user_id,
                username,
                original_status,
                custom_title,
                admin_rights_json,
                permissions_json,
                restore_at,
                now
            ],
        )?;

        Ok(conn.last_insert_rowid())
    }

    pub fn get_latest_active_backup(&self, chat_id: &str, user_id: i64) -> Result<Option<PermissionBackupRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, chat_id, user_id, username, original_status, custom_title,
                    admin_rights_json, permissions_json, restore_at, restored, created_at
             FROM permission_backups
             WHERE chat_id = ?1 AND user_id = ?2 AND restored = 0
             ORDER BY id DESC LIMIT 1"
        )?;

        let mut rows = stmt.query(params![chat_id, user_id])?;
        if let Some(row) = rows.next()? {
            let restored_int: i64 = row.get(9)?;
            Ok(Some(PermissionBackupRecord {
                id: row.get(0)?,
                chat_id: row.get(1)?,
                user_id: row.get(2)?,
                username: row.get(3)?,
                original_status: row.get(4)?,
                custom_title: row.get(5)?,
                admin_rights_json: row.get(6)?,
                permissions_json: row.get(7)?,
                restore_at: row.get(8)?,
                restored: restored_int != 0,
                created_at: row.get(10)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn mark_backup_restored(&self, backup_id: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE permission_backups SET restored = 1 WHERE id = ?1",
            params![backup_id],
        )?;
        Ok(())
    }

    pub fn get_pending_restorations(&self) -> Result<Vec<PermissionBackupRecord>> {
        let conn = self.conn.lock().unwrap();
        let now_ts = chrono::Utc::now().timestamp();
        let mut stmt = conn.prepare(
            "SELECT id, chat_id, user_id, username, original_status, custom_title,
                    admin_rights_json, permissions_json, restore_at, restored, created_at
             FROM permission_backups
             WHERE restored = 0 AND restore_at IS NOT NULL AND restore_at <= ?1
             ORDER BY id ASC"
        )?;

        let rows = stmt.query_map(params![now_ts], |row| {
            let restored_int: i64 = row.get(9)?;
            Ok(PermissionBackupRecord {
                id: row.get(0)?,
                chat_id: row.get(1)?,
                user_id: row.get(2)?,
                username: row.get(3)?,
                original_status: row.get(4)?,
                custom_title: row.get(5)?,
                admin_rights_json: row.get(6)?,
                permissions_json: row.get(7)?,
                restore_at: row.get(8)?,
                restored: restored_int != 0,
                created_at: row.get(10)?,
            })
        })?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r?);
        }
        Ok(list)
    }

    pub fn save_chat_message(
        &self,
        chat_id: &str,
        message_id: i64,
        user_id: i64,
        username: Option<&str>,
        full_name: &str,
        text: &str,
        reply_to_message_id: Option<i64>,
        reply_to_user_id: Option<i64>,
        reply_to_text: Option<&str>,
        timestamp: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Local::now().to_rfc3339();
        let clean_username = username.map(|u| u.trim_start_matches('@').to_string());

        conn.execute(
            "INSERT INTO chat_messages (
                chat_id, message_id, user_id, username, full_name, text,
                reply_to_message_id, reply_to_user_id, reply_to_text, timestamp, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(chat_id, message_id) DO UPDATE SET
                text = excluded.text,
                full_name = excluded.full_name,
                username = excluded.username",
            params![
                chat_id,
                message_id,
                user_id,
                clean_username,
                full_name,
                text,
                reply_to_message_id,
                reply_to_user_id,
                reply_to_text,
                timestamp,
                now
            ],
        )?;

        Ok(())
    }

    /// Получение истории сообщений чата в хронологическом порядке (от старых к новым)
    /// с фильтрацией по времени и ограничением по количеству сообщений и символов
    pub fn get_chat_history_since(
        &self,
        chat_id: &str,
        max_messages: usize,
        max_chars: usize,
        since_ts: Option<i64>,
    ) -> Result<Vec<ChatMessageRecord>> {
        let conn = self.conn.lock().unwrap();
        let query = if since_ts.is_some() {
            "SELECT id, chat_id, message_id, user_id, username, full_name, text,
                    reply_to_message_id, reply_to_user_id, reply_to_text, timestamp, created_at
             FROM chat_messages
             WHERE chat_id = ?1 AND timestamp > ?3
             ORDER BY timestamp DESC, id DESC
             LIMIT ?2"
        } else {
            "SELECT id, chat_id, message_id, user_id, username, full_name, text,
                    reply_to_message_id, reply_to_user_id, reply_to_text, timestamp, created_at
             FROM chat_messages
             WHERE chat_id = ?1
             ORDER BY timestamp DESC, id DESC
             LIMIT ?2"
        };

        let mut stmt = conn.prepare(query)?;
        let mut rows = if let Some(since) = since_ts {
            stmt.query(params![chat_id, max_messages as i64, since])?
        } else {
            stmt.query(params![chat_id, max_messages as i64])?
        };

        let mut raw_list = Vec::new();
        let mut total_chars = 0;
        while let Some(row) = rows.next()? {
            let item = ChatMessageRecord {
                id: row.get(0)?,
                chat_id: row.get(1)?,
                message_id: row.get(2)?,
                user_id: row.get(3)?,
                username: row.get(4)?,
                full_name: row.get(5)?,
                text: row.get(6)?,
                reply_to_message_id: row.get(7)?,
                reply_to_user_id: row.get(8)?,
                reply_to_text: row.get(9)?,
                timestamp: row.get(10)?,
                created_at: row.get(11)?,
            };
            total_chars += item.text.len();
            raw_list.push(item);
            if total_chars >= max_chars {
                break;
            }
        }

        // Переворачиваем обратно в хронологический порядок: от старых к новым
        raw_list.reverse();
        Ok(raw_list)
    }

    /// Получение недавней истории сообщений чата в хронологическом порядке (от старых к новым)
    pub fn get_chat_history(
        &self,
        chat_id: &str,
        max_messages: usize,
        max_chars: usize,
    ) -> Result<Vec<ChatMessageRecord>> {
        self.get_chat_history_since(chat_id, max_messages, max_chars, None)
    }

    /// Получение контекста для разбора спора (последние сообщения чата)
    pub fn get_recent_dispute_context(&self, chat_id: &str, limit: usize) -> Result<Vec<ChatMessageRecord>> {
        self.get_chat_history(chat_id, limit, 500_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_messages_storage_and_history() {
        let db = Database::open(":memory:").expect("Failed to open in-memory db");

        db.save_chat_message("-1001", 1, 10, Some("user1"), "Alice", "Первое сообщение", None, None, None, 100)
            .expect("save message 1");
        db.save_chat_message("-1001", 2, 20, Some("user2"), "Bob", "Второе сообщение в споре", Some(1), Some(10), Some("Первое сообщение"), 110)
            .expect("save message 2");
        db.save_chat_message("-1001", 3, 10, Some("user1"), "Alice", "Третье сообщение с аргументом", Some(2), Some(20), Some("Второе сообщение в споре"), 120)
            .expect("save message 3");

        // Проверяем получение истории (хронологический порядок: msg 1, 2, 3)
        let history = db.get_chat_history("-1001", 10, 10000).expect("get_chat_history");
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].message_id, 1);
        assert_eq!(history[0].text, "Первое сообщение");
        assert_eq!(history[1].message_id, 2);
        assert_eq!(history[1].reply_to_user_id, Some(10));
        assert_eq!(history[2].message_id, 3);
        assert_eq!(history[2].text, "Третье сообщение с аргументом");

        // Проверка лимита количества сообщений
        let history_limited = db.get_chat_history("-1001", 2, 10000).expect("get_chat_history limited");
        assert_eq!(history_limited.len(), 2);
        assert_eq!(history_limited[0].message_id, 2);
        assert_eq!(history_limited[1].message_id, 3);

        // Проверка обновления при конфликте (chat_id, message_id)
        db.save_chat_message("-1001", 3, 10, Some("user1"), "Alice", "Отредактированное третье сообщение", Some(2), Some(20), Some("Второе сообщение в споре"), 120)
            .expect("save message 3 edit");
        let history_updated = db.get_chat_history("-1001", 10, 10000).expect("get_chat_history after edit");
        assert_eq!(history_updated.len(), 3);
        assert_eq!(history_updated[2].text, "Отредактированное третье сообщение");

        // Проверка фильтрации истории с момента очистки (since_ts)
        let history_since = db.get_chat_history_since("-1001", 10, 10000, Some(105)).expect("get_chat_history_since");
        assert_eq!(history_since.len(), 2);
        assert_eq!(history_since[0].message_id, 2);
        assert_eq!(history_since[1].message_id, 3);

        let history_after_all = db.get_chat_history_since("-1001", 10, 10000, Some(130)).expect("get_chat_history_since after all");
        assert_eq!(history_after_all.len(), 0);

        // Проверка работы с настройками (set_setting, get_setting, delete_setting)
        db.set_setting("agy_conv_id_-1001", "test-uuid-1234").expect("set_setting");
        assert_eq!(db.get_setting("agy_conv_id_-1001").expect("get_setting"), Some("test-uuid-1234".to_string()));
        db.delete_setting("agy_conv_id_-1001").expect("delete_setting");
        assert_eq!(db.get_setting("agy_conv_id_-1001").expect("get_setting after delete"), None);

        // Проверка разбора спора
        let dispute_ctx = db.get_recent_dispute_context("-1001", 5).expect("get dispute context");
        assert_eq!(dispute_ctx.len(), 3);
    }
}

