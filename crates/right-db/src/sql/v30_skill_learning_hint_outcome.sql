-- v30: Persist probe-writer hint outcomes for learning finishes.
ALTER TABLE skill_learning_events
ADD COLUMN hint_outcome TEXT
CHECK (
    hint_outcome IS NULL
    OR hint_outcome IN ('applied_as_hinted', 'applied_differently', 'refused')
);
