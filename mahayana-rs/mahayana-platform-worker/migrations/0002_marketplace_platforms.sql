ALTER TABLE marketplace_plugins
ADD COLUMN platforms_json TEXT NOT NULL DEFAULT '["cli","desktop","mobile","web"]';
