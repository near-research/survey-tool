-- near-forms database schema
-- Single-form MVP with hardcoded form_id

CREATE TABLE forms (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    creator_id   TEXT NOT NULL,
    title        TEXT NOT NULL,
    questions    JSONB NOT NULL,
    created_at   TIMESTAMPTZ DEFAULT now()
);

CREATE TABLE submissions (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    form_id        UUID NOT NULL REFERENCES forms(id),
    submitter_id   TEXT NOT NULL,
    encrypted_blob TEXT NOT NULL,
    submitted_at   TIMESTAMPTZ DEFAULT now(),
    UNIQUE(form_id, submitter_id)
);

CREATE INDEX idx_submissions_form_time ON submissions(form_id, submitted_at DESC);
