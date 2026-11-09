#!/usr/bin/env python3
"""Generate owned MIME messages with encoded headers, alternatives and attachments."""
from datetime import datetime, timezone, timedelta
from email.message import EmailMessage
from email.policy import SMTP
from pathlib import Path


def make(path):
    message = EmailMessage(policy=SMTP)
    message['From'] = 'Dana Café <dana@example.test>'
    message['To'] = 'Riley <riley@example.test>'
    message['Cc'] = 'Morgan <morgan@example.test>'
    message['Reply-To'] = 'harbor@example.test'
    message['Subject'] = 'Harbor meeting – café'
    message['Date'] = datetime(2026, 11, 8, 9, 30, tzinfo=timezone(timedelta(hours=-6)))
    message['Message-ID'] = '<owned-harbor@example.test>'
    message.set_content('Meet beside the blue pier.\n\nBring the route notes.\n')
    message.add_alternative('''<html><head><style>HIDDEN STYLE</style></head><body><h1>Harbor plan</h1><p>Meet beside the <strong>blue</strong> pier &amp; café.</p><p>Bring the route notes.</p><script>HIDDEN SCRIPT</script><img src="https://example.invalid/tracker.png"/></body></html>''', subtype='html', cte='quoted-printable')
    message.add_attachment(b'Owned route notes.\n', maintype='text', subtype='plain', filename='route.txt')
    message.add_attachment(bytes(range(16)), maintype='application', subtype='octet-stream', filename='tide-data.bin')
    for index, part in enumerate(message.walk()):
        if part.is_multipart():
            part.set_boundary(f'owned-boundary-{index}')
    path.write_bytes(message.as_bytes())


if __name__ == '__main__':
    make(Path(__file__).resolve().parents[1] / 'crates/little-clip-core/tests/fixtures/email/harbor.eml')
