import type { PageKey } from '@/types';
import { ChatPage } from '@/pages/ChatPage';
import { HomePage } from '@/pages/HomePage';
import { DrawingPage } from '@/pages/DrawingPage';
import { MemoryPage } from '@/pages/MemoryPage';
import { FilesPage } from '@/pages/FilesPage';
import { SettingsPage } from '@/pages/SettingsPage';
import { SkillsPage } from '@/pages/SkillsPage';

interface ContentAreaProps {
  activePage: PageKey;
}

export function ContentArea({ activePage }: ContentAreaProps) {
  switch (activePage) {
    case 'home':
      return <HomePage />;
    case 'chat':
      return <ChatPage />;
    case 'drawing':
      return <DrawingPage />;
    case 'memory':
      return <MemoryPage />;
    case 'files':
      return <FilesPage />;
    case 'settings':
      return <SettingsPage />;
    case 'skills':
      return <SkillsPage />;
    case 'knowledge':
      return <ChatPage />;
    default: {
      const _exhaustive: never = activePage;
      throw new Error(`Unhandled page key: ${_exhaustive}`);
    }
  }
}
