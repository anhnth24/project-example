-- Expand: accent-fold-v1 FTS parity for chunk retrieval (P1B-R01 / G0-RET).
-- Lock/data risk: rewrites chunks.tsv via trigger-compatible UPDATE; no schema drop.
-- Rollback compatibility: function/trigger replaceable; previous raw-simple tsv regenerable.
-- Mirrors fileconv_core::intelligence::normalize_search_text (NFD strip + đ→d + lower).

CREATE OR REPLACE FUNCTION markhand_accent_fold(input text)
RETURNS text
LANGUAGE sql
IMMUTABLE
PARALLEL SAFE
AS $$
    SELECT lower(
        translate(
            regexp_replace(normalize(coalesce(input, ''), NFD), '[\u0300-\u036f]', '', 'g'),
            'đĐ',
            'dD'
        )
    );
$$;

CREATE OR REPLACE FUNCTION chunks_set_tsv()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    NEW.tsv := to_tsvector(
        'simple',
        markhand_accent_fold(
            coalesce(array_to_string(NEW.heading_path, ' '), '') || ' ' || NEW.body
        )
    );
    RETURN NEW;
END;
$$;

-- Backfill existing rows so query-side accent-fold-v1 matches stored vectors.
UPDATE chunks
SET body = body;
