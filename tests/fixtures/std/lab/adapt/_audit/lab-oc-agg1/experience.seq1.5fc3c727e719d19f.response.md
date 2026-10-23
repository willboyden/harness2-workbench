<edits>
component: scripts/summarize.py
candidates: inv1
change: dropped the positive-only filter from the amount sum
observed: the probe total included the credits the base run omitted
verdict: helped
</edits>

<experience>
cue: When a ledger includes credits and debits
lesson: Include both signs when summing in scripts/summarize.py.
evidence: The scripts/summarize.py row -- removing the positive-only filter changed the probe total to include the credits omitted by the base trajectory.
confidence: high for this observed arithmetic contrast
</experience>