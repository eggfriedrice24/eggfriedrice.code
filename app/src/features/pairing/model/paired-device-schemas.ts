// The schemas of the stored pairing records (plan section 2.3 item 9). The JSON is
// snake_case, as efrd names the same facts. Version 2 holds one record for each machine
// in one array, because expo-secure-store cannot list its keys. Version 1 held one record
// under another key; the store reads it once and moves it into version 2.

import { z } from "zod";

import { isTailnetIpv4 } from "./pairing-url";

/** The version of the stored list. */
export const RECORDS_VERSION = 2;

/** The version of the old single record. */
export const RECORD_V1_VERSION = 1;

/** The fields of one stored record, the same in version 1 and in version 2. */
const recordFields = {
  device_id: z.guid(),
  daemon_id: z.guid(),
  host: z.string().refine(isTailnetIpv4),
  port: z.int().min(1).max(65_535),
  fqdn: z.string().exactOptional(),
  name: z.string(),
  key_alias: z.string().regex(/^[A-Za-z0-9._-]{1,64}$/),
  scopes: z.array(z.string()),
  paired_at: z.string(),
};

/** One record in the version 2 list. */
export const storedRecordSchema = z.object(recordFields);

/** The stored JSON of one record. */
export type StoredRecord = z.infer<typeof storedRecordSchema>;

/**
 * The version 2 list. Each record is checked on its own (`storedRecordSchema`), so one
 * bad record does not lose the others.
 */
export const storedRecordsSchema = z.object({
  version: z.literal(RECORDS_VERSION),
  records: z.array(z.unknown()),
});

/** The version 1 record, under the old key. */
export const storedRecordV1Schema = z.object({
  version: z.literal(RECORD_V1_VERSION),
  ...recordFields,
});
