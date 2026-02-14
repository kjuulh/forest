-- Rename namespace column to organisation in projects table
ALTER TABLE projects RENAME COLUMN namespace TO organisation;

DROP INDEX IF EXISTS idx_project_namespace;
CREATE INDEX idx_project_organisation ON projects (organisation);

DROP INDEX IF EXISTS idx_project_unique;
CREATE UNIQUE INDEX idx_project_unique ON projects (organisation, project);

-- Ensure projects reference a valid organisation
ALTER TABLE projects
    ADD CONSTRAINT fk_projects_organisation
    FOREIGN KEY (organisation) REFERENCES organisations(name);

-- Add organisation to destinations
ALTER TABLE destinations ADD COLUMN organisation TEXT NOT NULL;
ALTER TABLE destinations
    ADD CONSTRAINT fk_destinations_organisation
    FOREIGN KEY (organisation) REFERENCES organisations(name);
CREATE INDEX idx_destinations_organisation ON destinations (organisation);

-- Rename namespace to organisation in components table
ALTER TABLE components RENAME COLUMN namespace TO organisation;
DROP INDEX IF EXISTS idx_component_unique_version;
CREATE UNIQUE INDEX idx_component_unique_version ON components (name, organisation, version);

-- Rename namespace to organisation in component_staging table
ALTER TABLE component_staging RENAME COLUMN namespace TO organisation;
DROP INDEX IF EXISTS idx_component_staging_unique_version;
CREATE UNIQUE INDEX idx_component_staging_unique_version ON component_staging (name, organisation, version);
