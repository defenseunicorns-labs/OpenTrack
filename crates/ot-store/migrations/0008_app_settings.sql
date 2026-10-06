-- Instance settings an admin changes from the Settings tab (site name,
-- classification banner): one JSON document, every save in the decision log.
CREATE TABLE app_settings (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    settings        TEXT    NOT NULL CHECK (json_valid(settings)),
    updated_at_ms   INTEGER NOT NULL,
    decision_id     INTEGER REFERENCES decisions(id)
);
