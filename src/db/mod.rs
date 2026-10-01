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

            CREATE INDEX IF NOT EXISTS idx_hw_entry_id ON processed_homeworks (homework_entry_student_id);
            CREATE INDEX IF NOT EXISTS idx_hw_status ON processed_homeworks (status);
            ",
        )
        .context("Ошибка создания таблиц SQLite")?;

        // Безопасное добавление новых колонок, если база уже существовала
        let _ = conn.execute("ALTER TABLE processed_homeworks ADD COLUMN is_pinned INTEGER DEFAULT 0", []);
        let _ = conn.execute("ALTER TABLE processed_homeworks ADD COLUMN pinned_at TEXT", []);
        let _ = conn.execute("ALTER TABLE processed_homeworks ADD COLUMN unpinned_at TEXT", []);

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
}
