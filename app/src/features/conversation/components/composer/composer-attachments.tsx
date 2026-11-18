// The files of the draft above the field (board S-Attach 3): the tile row, which appears
// with the first file, and under it the line of a file over the limit of efrd, in
// state_error. Send stays off while that line shows (use-composer.ts).

import { use } from "react";
import { View } from "react-native";

import { BodyText } from "@/components/ui/body-text";

import { overLimitLine } from "../../model/attachments-format";
import { overLimit } from "../../model/attachments-tiles";
import { AttachmentRow } from "../attachment-row";
import { ComposerContext } from "./composer-context";

/** The tiles of the draft, or nothing while it has none. */
export function ComposerAttachments() {
  const attach = use(ComposerContext)?.attach;
  if (attach === undefined || attach.tiles.length === 0) {
    return null;
  }
  const over = overLimit(attach.tiles);
  return (
    <View testID="composer-attachments" className="gap-6 px-12 pt-10">
      <AttachmentRow
        tiles={attach.tiles}
        onRemove={attach.remove}
        onLook={(tile) => attach.quickLook(tile.key)}
      />
      {over === undefined || over.bytes === undefined ? null : (
        <BodyText testID="composer-over-limit" size="caption" tone="state_error">
          {overLimitLine(over.name, over.bytes, attach.limits.max_file_bytes)}
        </BodyText>
      )}
    </View>
  );
}
