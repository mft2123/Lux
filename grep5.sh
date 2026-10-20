#!/bin/bash
cd /tmp/lux-u11/web/src/features/settings
echo "=== leftover references ==="
grep -n "setSaved\|errorMessage\|payloadFromDraft\|validateDraft\|save.mutate()" ArchiveSettingsSection.tsx || echo "none"
echo "=== footer count ==="
grep -c "SectionSaveBar" ArchiveSettingsSection.tsx
echo "=== group headers ==="
grep -n "<h3>" ArchiveSettingsSection.tsx