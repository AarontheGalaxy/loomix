## Summary

## Checklist

- [ ] No allocation, free, lock or I/O in the audio callback
- [ ] Golden files regenerated deliberately, and the diff reviewed, if `testdata/golden/` changed
- [ ] New behaviour covered by tests at the appropriate layer (spec section 4.1)
- [ ] `docs/` updated, including `docs/DSP.md` if a filter changed
- [ ] CHANGELOG entry added
- [ ] Manual smoke test recorded below: macOS version, audio interface, what was verified
