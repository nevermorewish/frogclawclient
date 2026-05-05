import { describe, expect, it } from 'vitest';
import {
  getStreamingLoadingState,
  hasFrogclawDisplayContent,
  hasModelVisibleContent,
  shouldRenderAssistantMarkdownFromContent,
  splitLeadingFrogclawDisplayContent,
  stripLeadingFrogclawDisplayTags,
} from '../chatStreaming';

describe('chat streaming helpers', () => {
  it('derives bubble and footer loading state from stream progress and content presence', () => {
    expect(getStreamingLoadingState(true, '')).toEqual({
      bubbleLoading: true,
      footerLoading: false,
    });

    expect(getStreamingLoadingState(true, 'hello')).toEqual({
      bubbleLoading: false,
      footerLoading: true,
    });

    expect(getStreamingLoadingState(false, 'hello')).toEqual({
      bubbleLoading: false,
      footerLoading: false,
    });
  });

  it('keeps streamed assistant messages on the content renderer after completion', () => {
    expect(shouldRenderAssistantMarkdownFromContent(true, false)).toBe(true);
    expect(shouldRenderAssistantMarkdownFromContent(false, true)).toBe(true);
    expect(shouldRenderAssistantMarkdownFromContent(false, false)).toBe(false);
  });

  it('ignores display-only tags when deciding whether model text exists', () => {
    const stripDisplayTags = (content: string) => content
      .replace(/<knowledge-retrieval [^>]*data-frogclaw="1"[^>]*>[\s\S]*?<\/knowledge-retrieval>\s*/g, '')
      .replace(/<think[^>]*>[\s\S]*?<\/think>\s*/g, '')
      .trim();

    expect(hasModelVisibleContent(
      '<knowledge-retrieval status="done" data-frogclaw="1">[]</knowledge-retrieval>',
      stripDisplayTags,
    )).toBe(false);
    expect(hasModelVisibleContent(
      '<knowledge-retrieval status="done" data-frogclaw="1">[]</knowledge-retrieval>\n\nanswer',
      stripDisplayTags,
    )).toBe(true);
  });

  it('detects FrogClaw display tags independently from model text', () => {
    expect(hasFrogclawDisplayContent(
      '<knowledge-retrieval status="done" data-frogclaw="1">[]</knowledge-retrieval>',
    )).toBe(true);
    expect(hasFrogclawDisplayContent('answer')).toBe(false);
  });

  it('splits leading FrogClaw display tags from streamed model text', () => {
    const knowledge = '<knowledge-retrieval status="done" data-frogclaw="1">[]</knowledge-retrieval>\n\n';
    const memory = '<memory-retrieval status="done" data-frogclaw="1">[]</memory-retrieval>\n\n';

    expect(splitLeadingFrogclawDisplayContent(`${knowledge}${memory}answer`)).toEqual({
      prefix: `${knowledge}${memory}`,
      body: 'answer',
    });
    expect(splitLeadingFrogclawDisplayContent(`answer\n${knowledge}`)).toEqual({
      prefix: '',
      body: `answer\n${knowledge}`,
    });
  });

  it('strips selected leading display tags while preserving other display prefixes', () => {
    const web = '<web-search status="done" data-frogclaw="1">[]</web-search>\n\n';
    const knowledge = '<knowledge-retrieval status="done" data-frogclaw="1">[]</knowledge-retrieval>\n\n';

    expect(stripLeadingFrogclawDisplayTags(
      `${web}${knowledge}answer`,
      ['knowledge-retrieval', 'memory-retrieval'],
    )).toBe(`${web}answer`);
  });
});
