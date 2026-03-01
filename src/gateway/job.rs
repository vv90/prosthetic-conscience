use uuid::Uuid;

pub struct JobId(pub Uuid);

pub struct Job {
    job_id: JobId,
    data: String,
}
