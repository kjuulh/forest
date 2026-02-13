-- Enable pg_trgm for trigram-based fuzzy / substring search.
create extension if not exists pg_trgm;

-- GIN trigram indexes for user search (username + email).
create index idx_users_username_trgm on users using gin(username gin_trgm_ops);
create index idx_user_emails_email_trgm on user_emails using gin(email gin_trgm_ops);
