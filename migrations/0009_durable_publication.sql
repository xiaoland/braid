ALTER TABLE write_intents ADD COLUMN public_request_id TEXT;
ALTER TABLE write_intents ADD COLUMN source_binding TEXT;
UPDATE write_intents SET public_request_id='request-' || lower(hex(randomblob(16)))
WHERE request_key IS NOT NULL;
CREATE UNIQUE INDEX write_intents_public_request ON write_intents(public_request_id);
ALTER TABLE github_write_outbox ADD COLUMN claim_owner TEXT;
ALTER TABLE github_write_outbox ADD COLUMN claim_generation INTEGER;
-- Legacy CLI obligations join the same executor. Sending may already have mutated.
INSERT INTO github_write_outbox(intent_id,repository,target_kind,target_database_id,
 operation,content,request_digest,remote_database_id,lifecycle,attempts,next_attempt_at,
 last_error,created_at,updated_at)
SELECT intent_id,repository,'agent',target,'agent_write',payload,
 lower(hex(randomblob(32))),remote_database_id,
 CASE WHEN lifecycle='sending' THEN 'uncertain' ELSE lifecycle END,
 attempts,updated_at,last_error,created_at,updated_at FROM write_intents
WHERE request_key IS NOT NULL AND lifecycle IN ('pending','sending','uncertain');
