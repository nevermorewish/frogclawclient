import { theme } from 'antd';
import ProjectMemorySettings from '@/components/settings/ProjectMemorySettings';

export function MemoryPage() {
  const { token } = theme.useToken();

  return (
    <div className="h-full" style={{ overflow: 'hidden', backgroundColor: token.colorBgElevated }}>
      <ProjectMemorySettings />
    </div>
  );
}
