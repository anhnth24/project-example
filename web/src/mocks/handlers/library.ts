import { registerOperation } from '../registry';
import { notFound, conflict as conflictResponse, apiError, unauthorized } from '../apiError';
import { nextRequestId, encodeCursor, decodeCursor, mockTimestamp } from '../ids';
import { authContextForHeader, getStore, nextId } from '../fixtures';
import type { components } from '../../api/generated/contract';

type Collection = components['schemas']['Collection'];
type CreateCollectionRequest = components['schemas']['CreateCollectionRequest'];
type UpdateCollectionRequest = components['schemas']['UpdateCollectionRequest'];
type Document = components['schemas']['Document'];
type Job = components['schemas']['Job'];

// ---------------------------------------------------------------------------
// Collections
// ---------------------------------------------------------------------------

// Org scoping (P2-06/P2-15 org switch): `Collection`'s wire shape has no
// `orgId` field — the real server isolates tenants from the bearer token,
// never a response field — so filtering here goes through
// `getStore().collectionOrgId` (collectionId -> orgId), keyed off whichever
// org the caller's *current* access token is scoped to
// (`authContextForHeader(...).orgId`, not any fixed `user.orgId`). This is
// what makes switching orgs actually change what `listCollections` returns
// instead of merely changing the bearer token's claimed org while serving
// the same fixed list.
registerOperation('listCollections', (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const items = getStore().collections.filter(
    (c) => getStore().collectionOrgId.get(c.id) === auth.orgId,
  );
  return { status: 200, body: { items, page: { hasMore: false, nextCursor: null } } };
});

registerOperation('createCollection', async (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const body = await ctx.json<CreateCollectionRequest>();
  const collection: Collection = {
    id: nextId(),
    name: body.name,
    slug: body.slug,
    description: body.description ?? null,
    visibility: body.visibility ?? 'org',
    createdAt: mockTimestamp(0),
  };
  getStore().collections.push(collection);
  getStore().collectionOrgId.set(collection.id, auth.orgId);
  return { status: 201, body: collection };
});

registerOperation('getCollection', (ctx) => {
  const auth = authContextForHeader(ctx.headers.get('authorization'));
  if (!auth) return unauthorized();
  const collection = getStore().collections.find((c) => c.id === ctx.params.collectionId);
  if (!collection || getStore().collectionOrgId.get(collection.id) !== auth.orgId) {
    return notFound(`Collection ${ctx.params.collectionId} does not exist.`);
  }
  return { status: 200, body: collection };
});

registerOperation('updateCollection', async (ctx) => {
  const collection = getStore().collections.find((c) => c.id === ctx.params.collectionId);
  if (!collection) return notFound(`Collection ${ctx.params.collectionId} does not exist.`);
  const body = await ctx.json<UpdateCollectionRequest>();
  collection.name = body.name;
  if ('description' in body) collection.description = body.description ?? null;
  return { status: 200, body: collection };
});

registerOperation('deleteCollection', (ctx) => {
  const store = getStore();
  const idx = store.collections.findIndex((c) => c.id === ctx.params.collectionId);
  if (idx === -1) return notFound(`Collection ${ctx.params.collectionId} does not exist.`);
  store.collections.splice(idx, 1);
  return { status: 204 };
});

// ---------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------

function findDocument(
  documentId: string,
): { document: Document; collectionId: string } | undefined {
  for (const [collectionId, docs] of getStore().documents) {
    const document = docs.find((d) => d.id === documentId);
    if (document) return { document, collectionId };
  }
  return undefined;
}

registerOperation('listDocuments', (ctx) => {
  const collectionId = ctx.params.collectionId;
  if (!getStore().collections.some((c) => c.id === collectionId)) {
    return notFound(`Collection ${collectionId} does not exist.`);
  }
  const all = getStore().documents.get(collectionId) ?? [];
  const rawLimit = ctx.query.get('limit');
  const limit = rawLimit ? Math.min(100, Math.max(1, Number(rawLimit))) : 50;
  const offset = decodeCursor(ctx.query.get('cursor'));
  const page = all.slice(offset, offset + limit);
  const hasMore = offset + limit < all.length;
  return {
    status: 200,
    body: {
      items: page,
      page: { hasMore, nextCursor: hasMore ? encodeCursor(offset + limit) : null },
    },
  };
});

registerOperation('approveIntake', async (ctx) => {
  const { collectionId, documentId } = ctx.params;
  const found = findDocument(documentId);
  if (!found || found.collectionId !== collectionId) {
    return notFound(`Document ${documentId} does not exist in collection ${collectionId}.`);
  }
  await ctx.json<{ reason?: string } | undefined>().catch(() => undefined);
  const versionId = found.document.currentVersionId ?? nextId();
  const jobId = nextId();
  const job: Job = {
    id: jobId,
    jobType: 'convert',
    status: 'pending',
    attempts: 0,
    documentId,
    versionId,
    createdAt: mockTimestamp(0),
    updatedAt: mockTimestamp(0),
    finishedAt: null,
  };
  getStore().jobs.set(jobId, job);
  return {
    status: 200,
    body: { documentId, versionId, collectionId, jobId, created: true, requestId: nextRequestId() },
  };
});

registerOperation('getDocument', (ctx) => {
  const found = findDocument(ctx.params.documentId);
  if (!found) return notFound(`Document ${ctx.params.documentId} does not exist.`);
  return { status: 200, body: found.document };
});

registerOperation('deleteDocument', (ctx) => {
  for (const docs of getStore().documents.values()) {
    const idx = docs.findIndex((d) => d.id === ctx.params.documentId);
    if (idx !== -1) {
      docs.splice(idx, 1);
      return { status: 204 };
    }
  }
  return notFound(`Document ${ctx.params.documentId} does not exist.`);
});

registerOperation('previewDocument', (ctx) => {
  const found = findDocument(ctx.params.documentId);
  if (!found) return notFound(`Document ${ctx.params.documentId} does not exist.`);
  const versions = getStore().versions.get(ctx.params.documentId) ?? [];
  const requestedVersionId = ctx.query.get('versionId');
  const version = requestedVersionId
    ? versions.find((v) => v.id === requestedVersionId)
    : versions.find((v) => v.isCurrent);
  if (!version) return notFound(`No matching version for document ${ctx.params.documentId}.`);
  return {
    status: 200,
    body: {
      documentId: found.document.id,
      versionId: version.id,
      versionNumber: version.versionNumber,
      sourceContentSha256: version.sourceContentSha256,
      canonicalMarkdownSha256: `md-${version.sourceContentSha256}`,
      isCurrent: version.isCurrent,
      truncated: false,
      markdown: `# ${found.document.title}\n\nMock preview content for version ${version.versionNumber}.`,
      requestId: nextRequestId(),
    },
  };
});

registerOperation('listDocumentVersions', (ctx) => {
  const versions = getStore().versions.get(ctx.params.documentId) ?? [];
  return { status: 200, body: { items: versions, page: { hasMore: false, nextCursor: null } } };
});

registerOperation('getDocumentVersion', (ctx) => {
  const versions = getStore().versions.get(ctx.params.documentId) ?? [];
  const version = versions.find((v) => v.id === ctx.params.versionId);
  if (!version) return notFound(`Version ${ctx.params.versionId} does not exist.`);
  return { status: 200, body: version };
});

registerOperation('diffDocumentVersions', (ctx) => {
  const versions = getStore().versions.get(ctx.params.documentId) ?? [];
  const left = versions.find((v) => v.id === ctx.params.versionId);
  const against = ctx.query.get('against');
  const right = versions.find((v) => v.id === against);
  if (!left || !right) return notFound('One or both versions in the diff do not exist.');
  return {
    status: 200,
    body: {
      documentId: ctx.params.documentId,
      left,
      right,
      note: 'Identity diff only; not a text diff.',
      requestId: nextRequestId(),
    },
  };
});

registerOperation('publishDocumentVersion', (ctx) => {
  const versions = getStore().versions.get(ctx.params.documentId) ?? [];
  const version = versions.find((v) => v.id === ctx.params.versionId);
  if (version) {
    versions.forEach((v) => (v.isCurrent = v.id === version.id));
    const found = findDocument(ctx.params.documentId);
    if (found) found.document.currentVersionId = version.id;
  }
  return { status: 204 };
});

registerOperation('issueDownloadCapability', async (ctx) => {
  const versions = getStore().versions.get(ctx.params.documentId) ?? [];
  const version = versions.find((v) => v.id === ctx.params.versionId);
  if (!version) return notFound(`Version ${ctx.params.versionId} does not exist.`);
  const body = await ctx.json<{ purpose: 'markdown' | 'original' }>();
  const capability = nextId();
  const expiresIn = 300;
  getStore().downloadCapabilities.set(capability, {
    capability,
    documentId: ctx.params.documentId,
    versionId: ctx.params.versionId,
    purpose: body.purpose,
    expiresAt: Date.now() + expiresIn * 1000,
    redeemed: false,
  });
  return {
    status: 200,
    body: {
      capability,
      expiresIn,
      purpose: body.purpose,
      documentId: ctx.params.documentId,
      versionId: ctx.params.versionId,
      requestId: nextRequestId(),
    },
  };
});

registerOperation('redeemDownload', (ctx) => {
  const record = getStore().downloadCapabilities.get(ctx.params.capability);
  if (!record || record.redeemed || record.expiresAt < Date.now()) {
    return notFound('Download capability is unknown, already redeemed, or expired.');
  }
  record.redeemed = true;
  const found = findDocument(record.documentId);
  const title = found?.document.title ?? 'document';
  if (record.purpose === 'markdown') {
    return {
      status: 200,
      rawBody: {
        text: `# ${title}\n\nMock canonical markdown for download.`,
        contentType: 'text/markdown; charset=utf-8',
      },
      headers: { 'Content-Disposition': 'attachment' },
    };
  }
  return {
    status: 200,
    rawBody: { text: `mock original bytes for ${title}`, contentType: 'application/octet-stream' },
    headers: { 'Content-Disposition': 'attachment' },
  };
});

registerOperation('reindexDocument', (ctx) => {
  // Spec declares only 200/429 for this operation (no 404) — enqueue regardless
  // of whether the id is known, matching the declared contract exactly.
  const jobId = nextId();
  const found = findDocument(ctx.params.documentId);
  const job: Job = {
    id: jobId,
    jobType: 'index',
    status: 'pending',
    attempts: 0,
    documentId: ctx.params.documentId,
    versionId: found?.document.currentVersionId ?? null,
    createdAt: mockTimestamp(0),
    updatedAt: mockTimestamp(0),
    finishedAt: null,
  };
  getStore().jobs.set(jobId, job);
  return {
    status: 200,
    body: {
      jobId,
      created: true,
      documentId: ctx.params.documentId,
      versionId: found?.document.currentVersionId ?? null,
      requestId: nextRequestId(),
    },
  };
});

// ---------------------------------------------------------------------------
// Citations
// ---------------------------------------------------------------------------

registerOperation('resolveCitation', async (ctx) => {
  const body = await ctx.json<components['schemas']['ResolveCitationRequest']>();
  const found = findDocument(body.logicalDocumentId);
  if (!found) return notFound(`Document ${body.logicalDocumentId} does not exist.`);
  return {
    status: 200,
    body: {
      citation: {
        citeId: nextId(),
        logicalDocumentId: body.logicalDocumentId,
        versionId: body.versionId,
        versionNumber: 1,
        sourceContentSha256: body.sourceContentSha256,
        canonicalMarkdownSha256: body.canonicalMarkdownSha256,
        quoteSha256: `quote-${body.chunkId}`,
        chunkId: body.chunkId,
        chunkIdentitySha256: `chunk-${body.chunkId}`,
        page: null,
        slide: null,
        sheet: null,
        sourceSpanStart: body.sourceSpanStart,
        sourceSpanEnd: body.sourceSpanEnd,
        quoteLocalStart: body.quoteLocalStart,
        quoteLocalEnd: body.quoteLocalEnd,
        quote: body.quote,
        isCurrent: true,
        anchor: `#${body.chunkId}`,
      },
      requestId: nextRequestId(),
    },
  };
});

// ---------------------------------------------------------------------------
// Conflicts
// ---------------------------------------------------------------------------

registerOperation('listConflicts', () => ({
  status: 200,
  body: { items: getStore().conflicts, requestId: nextRequestId() },
}));

registerOperation('getConflict', (ctx) => {
  const found = getStore().conflicts.find((c) => c.id === ctx.params.conflictId);
  if (!found) return notFound(`Conflict ${ctx.params.conflictId} does not exist.`);
  return { status: 200, body: { ...found, requestId: nextRequestId() } };
});

registerOperation('getConflictEvidence', (ctx) => {
  const found = getStore().conflicts.find((c) => c.id === ctx.params.conflictId);
  if (!found) return notFound(`Conflict ${ctx.params.conflictId} does not exist.`);
  return {
    status: 200,
    body: {
      conflictId: found.id,
      status: found.status,
      resolutionNote: found.resolutionNote,
      resolvedAt: found.resolvedAt,
      items: [
        { legId: nextId(), collectionId: found.collectionAId, claimId: found.claimAId },
        { legId: nextId(), collectionId: found.collectionBId, claimId: found.claimBId },
      ],
      requestId: nextRequestId(),
    },
  };
});

registerOperation('triageConflict', async (ctx) => {
  const found = getStore().conflicts.find((c) => c.id === ctx.params.conflictId);
  if (!found) return notFound(`Conflict ${ctx.params.conflictId} does not exist.`);
  const body = await ctx.json<{ status: string; resolutionNote?: string }>();
  found.status = body.status;
  found.resolutionNote = body.resolutionNote ?? null;
  found.resolvedAt = mockTimestamp(0);
  return {
    status: 200,
    body: {
      id: found.id,
      status: found.status,
      resolvedAt: found.resolvedAt,
      requestId: nextRequestId(),
    },
  };
});

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

registerOperation('getJob', (ctx) => {
  const job = getStore().jobs.get(ctx.params.jobId);
  if (!job) return notFound(`Job ${ctx.params.jobId} does not exist.`);
  return { status: 200, body: job };
});

// ---------------------------------------------------------------------------
// Uploads
// ---------------------------------------------------------------------------

registerOperation('createUpload', async (ctx) => {
  const form = await ctx.formData();
  const file = form.get('file');
  const collectionId = form.get('collectionId');
  const documentId = form.get('documentId');
  // Not `instanceof File`: under vitest's jsdom test environment, the `File`
  // global visible to test/handler code (jsdom's) and the `File` class the
  // Node-native `fetch`/`FormData` machinery actually constructs (undici's)
  // are two different classes, so `instanceof` fails even for a genuine file
  // part. `file` is a `File | string` per the DOM lib types, so checking it's
  // not a plain string distinguishes "file part" from "text field" just as
  // reliably, without depending on which realm's File constructor built it.
  if (typeof file === 'string' || file === null || typeof collectionId !== 'string') {
    return { status: 400, body: apiError('bad_request', 'file and collectionId are required.') };
  }
  if (!getStore().collections.some((c) => c.id === collectionId)) {
    return notFound(`Collection ${collectionId} does not exist.`);
  }
  const sha256 = `sha256-${file.name}-${file.size}`;
  const isQuarantined = file.name.toLowerCase().startsWith('quarantine-');

  if (typeof documentId === 'string') {
    const found = findDocument(documentId);
    if (!found) return notFound(`Document ${documentId} does not exist.`);
    if (found.document.state !== 'indexed') {
      return conflictResponse(
        `Document ${documentId} is not fully indexed; cannot accept a new revision yet.`,
      );
    }
    const versions = getStore().versions.get(documentId) ?? [];
    const versionId = nextId();
    const versionNumber = versions.length + 1;
    versions.forEach((v) => (v.isCurrent = false));
    versions.push({
      id: versionId,
      documentId,
      versionNumber,
      isCurrent: true,
      sourceContentSha256: sha256,
      effectiveFrom: mockTimestamp(0),
      effectiveTo: null,
      changeSummary: 'New revision uploaded.',
      createdAt: mockTimestamp(0),
    });
    getStore().versions.set(documentId, versions);
    found.document.currentVersionId = versionId;
    found.document.state = 'converting';
    const jobId = nextId();
    getStore().jobs.set(jobId, {
      id: jobId,
      jobType: 'convert',
      status: 'pending',
      attempts: 0,
      documentId,
      versionId,
      createdAt: mockTimestamp(0),
      updatedAt: mockTimestamp(0),
      finishedAt: null,
    });
    return {
      status: 201,
      body: {
        disposition: 'accepted',
        objectId: nextId(),
        documentId,
        versionId,
        jobId,
        collectionId,
        sha256,
        sizeBytes: file.size,
        canonicalFormat: 'markdown',
        requestId: nextRequestId(),
      },
    };
  }

  const newDocumentId = nextId();
  const newVersionId = nextId();
  const newDoc: Document = {
    id: newDocumentId,
    collectionId,
    title: file.name,
    state: isQuarantined ? 'uploaded' : 'converting',
    currentVersionId: isQuarantined ? null : newVersionId,
    createdAt: mockTimestamp(0),
    updatedAt: mockTimestamp(0),
  };
  const docs = getStore().documents.get(collectionId) ?? [];
  docs.push(newDoc);
  getStore().documents.set(collectionId, docs);
  getStore().versions.set(newDocumentId, [
    {
      id: newVersionId,
      documentId: newDocumentId,
      versionNumber: 1,
      isCurrent: !isQuarantined,
      sourceContentSha256: sha256,
      effectiveFrom: mockTimestamp(0),
      effectiveTo: null,
      changeSummary: null,
      createdAt: mockTimestamp(0),
    },
  ]);

  let jobId: string | undefined;
  if (!isQuarantined) {
    jobId = nextId();
    getStore().jobs.set(jobId, {
      id: jobId,
      jobType: 'convert',
      status: 'pending',
      attempts: 0,
      documentId: newDocumentId,
      versionId: newVersionId,
      createdAt: mockTimestamp(0),
      updatedAt: mockTimestamp(0),
      finishedAt: null,
    });
  }

  return {
    status: 201,
    body: {
      disposition: isQuarantined ? 'quarantined' : 'accepted',
      objectId: nextId(),
      documentId: newDocumentId,
      versionId: newVersionId,
      jobId,
      collectionId,
      sha256,
      sizeBytes: file.size,
      canonicalFormat: 'markdown',
      requestId: nextRequestId(),
    },
  };
});
