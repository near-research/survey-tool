-- Add NOT NULL constraints to timestamp columns.
-- Existing rows already have non-null values (populated by DEFAULT now() or explicit NOW()).

ALTER TABLE forms ALTER COLUMN created_at SET NOT NULL;
ALTER TABLE submissions ALTER COLUMN submitted_at SET NOT NULL;
