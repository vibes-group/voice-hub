// @vitest-environment jsdom
import 'fake-indexeddb/auto';
import { beforeEach, describe, expect, it } from 'vitest';
import { useStore, type ChatMessage } from './useStore';

function fileMsg(id: string, uploadIds: string[]): ChatMessage {
  return {
    id,
    from: 'peer',
    text: '',
    ts: Date.now(),
    attachments: uploadIds.map((uploadId) => ({
      uploadId,
      kind: 'file',
      name: uploadId,
      mime: 'application/octet-stream',
      size: 1,
    })),
  };
}

beforeEach(() => {
  localStorage.clear();
  useStore.setState({ chatByRoom: {} });
});

describe('markAttachmentsDeleted', () => {
  it('flags matching attachments across rooms and persists only changed rooms', () => {
    useStore.setState({
      chatByRoom: { room1: [fileMsg('m1', ['a'])], room2: [fileMsg('m2', ['b'])] },
    });

    useStore.getState().markAttachmentsDeleted(['a']);

    expect(useStore.getState().chatByRoom.room1[0].deletedUploadIds).toEqual(['a']);
    expect(useStore.getState().chatByRoom.room2[0].deletedUploadIds).toBeUndefined();

    const persisted = JSON.parse(localStorage.getItem('voice-hub.chat.room1')!);
    expect(persisted[0].deletedUploadIds).toEqual(['a']);
    // The unaffected room is not rewritten.
    expect(localStorage.getItem('voice-hub.chat.room2')).toBeNull();
  });

  it('is a no-op when the ids are already marked (state untouched)', () => {
    const m = fileMsg('m1', ['a']);
    m.deletedUploadIds = ['a'];
    useStore.setState({ chatByRoom: { room1: [m] } });
    const before = useStore.getState().chatByRoom.room1;

    useStore.getState().markAttachmentsDeleted(['a']);

    expect(useStore.getState().chatByRoom.room1).toBe(before);
  });

  it('ignores ids not present in any message', () => {
    useStore.setState({ chatByRoom: { room1: [fileMsg('m1', ['a'])] } });
    useStore.getState().markAttachmentsDeleted(['unknown']);
    expect(useStore.getState().chatByRoom.room1[0].deletedUploadIds).toBeUndefined();
  });

  it('merges into existing deletions without duplicating', () => {
    const m = fileMsg('m1', ['a', 'b']);
    m.deletedUploadIds = ['a'];
    useStore.setState({ chatByRoom: { room1: [m] } });

    useStore.getState().markAttachmentsDeleted(['a', 'b']);

    expect(useStore.getState().chatByRoom.room1[0].deletedUploadIds).toEqual(['a', 'b']);
  });
});
