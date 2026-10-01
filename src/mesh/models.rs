use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomeworksShortResponse {
    pub payload: Option<Vec<HomeworkItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomeworkItem {
    pub homework_entry_student_id: i64,
    pub lesson_id: Option<i64>,
    pub subject_id: Option<i64>,
    pub subject_name: Option<String>,
    pub description: Option<String>,
    pub date: Option<String>,
    pub date_assigned_on: Option<String>,
    pub lesson_date_time: Option<String>,
    pub is_done: Option<bool>,
    pub has_written_answer: Option<bool>,
    #[serde(rename = "type")]
    pub item_type: Option<String>,
    pub attachments: Option<Vec<HomeworkAttachment>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomeworkAttachment {
    pub id: Option<i64>,
    pub file_id: Option<i64>,
    pub name: Option<String>,
    pub file_name: Option<String>,
    pub link: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileResponse {
    pub id: Option<i64>,
    pub profile_id: Option<i64>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub middle_name: Option<String>,
    pub short_name: Option<String>,
    pub children: Option<Vec<ProfileChild>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileChild {
    pub id: Option<i64>,
    pub contingent_guid: Option<String>,
    pub person_id: Option<String>,
    pub class_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventCalendarResponse {
    pub total_count: Option<i64>,
    pub response: Option<Vec<EventCalendarItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventCalendarItem {
    pub id: Option<i64>,
    pub start_at: Option<String>,
    pub finish_at: Option<String>,
    pub cancelled: Option<bool>,
    pub replaced: Option<bool>,
    pub is_missed_lesson: Option<bool>,
    pub room_name: Option<String>,
    pub room_number: Option<String>,
    pub subject_id: Option<i64>,
    pub subject_name: Option<String>,
    pub teacher: Option<EventTeacher>,
    pub author: Option<EventTeacher>,
    pub homework: Option<EventHomework>,
    pub marks: Option<Vec<EventMark>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventTeacher {
    pub last_name: Option<String>,
    pub first_name: Option<String>,
    pub middle_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventHomework {
    pub descriptions: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMark {
    pub value: Option<String>,
    pub weight: Option<f64>,
}
