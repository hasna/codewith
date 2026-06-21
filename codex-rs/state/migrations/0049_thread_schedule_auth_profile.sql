ALTER TABLE thread_schedules
    ADD COLUMN auth_profile_recorded INTEGER NOT NULL DEFAULT 0 CHECK(auth_profile_recorded IN (0, 1));

ALTER TABLE thread_schedules
    ADD COLUMN auth_profile TEXT;
