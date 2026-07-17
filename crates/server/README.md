# fileconv-server

Future Markhand Web API and worker boundary. Phase F contains only compileable binaries
for help/config validation; it intentionally has no HTTP framework, database, auth,
route or job implementation.

Later code follows `route → service → repository/adapter`; business operations require
an explicit `OrgContext`.
