import { App } from 'antd';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ChatImageNode } from '../ChatImageNode';

const imageActions = vi.hoisted(() => ({
  copyChatImage: vi.fn(async (_src: string) => undefined),
  saveChatImage: vi.fn(async (_src: string, _defaultName?: string) => true),
  getDefaultImageFilename: vi.fn((_src: string, _alt?: string | null) => 'frogclaw-image.png'),
}));

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (_key: string, fallback?: string) => fallback ?? _key,
  }),
}));

vi.mock('@/lib/chatImageActions', () => ({
  copyChatImage: (src: string) => imageActions.copyChatImage(src),
  saveChatImage: (src: string, defaultName?: string) => imageActions.saveChatImage(src, defaultName),
  getDefaultImageFilename: (src: string, alt?: string | null) => imageActions.getDefaultImageFilename(src, alt),
}));

function renderNode(src = 'data:image/png;base64,aGVsbG8=') {
  return render(
    <App>
      <ChatImageNode
        node={{
          type: 'image',
          src,
          alt: 'generated preview',
          title: null,
          raw: `![generated preview](${src})`,
        }}
      />
    </App>,
  );
}

describe('ChatImageNode', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    imageActions.copyChatImage.mockResolvedValue(undefined);
    imageActions.saveChatImage.mockResolvedValue(true);
  });

  it('shows a loading region until the image finishes loading', () => {
    renderNode();

    expect(screen.getByText('图片加载中...')).toBeInTheDocument();

    fireEvent.load(screen.getByAltText('generated preview'));

    expect(screen.queryByText('图片加载中...')).not.toBeInTheDocument();
  });

  it('renders an antd error alert with the image link when loading fails', () => {
    const src = 'https://example.com/missing.png';
    renderNode(src);

    fireEvent.error(screen.getByAltText('generated preview'));

    expect(screen.getByText('图片加载失败')).toBeInTheDocument();
    expect(screen.getByText(src)).toBeInTheDocument();
  });

  it('reads image details from html img node attributes', () => {
    const src = 'https://example.com/generated.webp';
    render(
      <App>
        <ChatImageNode
          node={{
            type: 'img',
            attrs: [
              ['src', src],
              ['alt', 'html preview'],
            ],
          }}
        />
      </App>,
    );

    expect(screen.getByText('图片加载中...')).toBeInTheDocument();

    fireEvent.load(screen.getByAltText('html preview'));

    expect(screen.queryByText('图片加载中...')).not.toBeInTheDocument();
  });

  it('copies and saves a loaded image through the image actions', async () => {
    const user = userEvent.setup();
    const src = 'data:image/png;base64,aGVsbG8=';
    renderNode(src);

    fireEvent.load(screen.getByAltText('generated preview'));

    await user.click(screen.getByRole('button', { name: '复制图片' }));
    await waitFor(() => expect(imageActions.copyChatImage).toHaveBeenCalledWith(src));

    await user.click(screen.getByRole('button', { name: '保存图片' }));
    await waitFor(() => expect(imageActions.saveChatImage).toHaveBeenCalledWith(src, 'frogclaw-image.png'));
  });
});
