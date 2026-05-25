UPDATE plan_runs
SET state = 'finished'
WHERE state IN ('succeeded', 'succeeded_empty', 'failed');
