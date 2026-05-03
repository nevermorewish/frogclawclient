import { useState } from 'react';
import { App, Button, Form, Input, Modal, Space, Typography } from 'antd';
import { CheckCircle2, LogIn, UserRound } from 'lucide-react';
import { useFrogclawAuthStore } from '@/stores/frogclawAuthStore';
import { useProviderStore } from '@/stores/providerStore';

interface LoginDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function LoginDialog({ open, onOpenChange }: LoginDialogProps) {
  const { message } = App.useApp();
  const [form] = Form.useForm<{ username: string; password: string }>();
  const login = useFrogclawAuthStore((state) => state.login);
  const fetchProviders = useProviderStore((state) => state.fetchProviders);
  const [loading, setLoading] = useState(false);
  const [loginSuccess, setLoginSuccess] = useState(false);

  const close = () => {
    onOpenChange(false);
    window.setTimeout(() => {
      form.resetFields();
      setLoginSuccess(false);
    }, 180);
  };

  const submit = async () => {
    const values = await form.validateFields();
    setLoading(true);
    try {
      await login(values.username, values.password);
      await fetchProviders();
      setLoginSuccess(true);
      message.success('已登录 FrogClaw，并同步供应商令牌');
    } catch (error) {
      message.error(`登录失败：${String(error)}`);
    } finally {
      setLoading(false);
    }
  };

  return (
    <Modal
      title={loginSuccess ? '登录成功' : '登录 FrogClaw'}
      open={open}
      onCancel={close}
      footer={loginSuccess ? (
        <Button type="primary" block onClick={close}>完成</Button>
      ) : (
        <Button type="primary" block loading={loading} icon={!loading ? <LogIn size={14} /> : undefined} onClick={submit}>
          登录
        </Button>
      )}
      width={380}
      destroyOnHidden
    >
      {loginSuccess ? (
        <Space direction="vertical" size={12} style={{ width: '100%', alignItems: 'center', padding: '12px 0' }}>
          <CheckCircle2 size={34} color="#22c55e" />
          <Typography.Text type="secondary">令牌和供应商配置已同步到本地。</Typography.Text>
        </Space>
      ) : (
        <Form form={form} layout="vertical" style={{ paddingTop: 8 }}>
          <Form.Item name="username" label="用户名" rules={[{ required: true, message: '请输入用户名' }]}>
            <Input prefix={<UserRound size={14} />} placeholder="请输入用户名" disabled={loading} autoFocus />
          </Form.Item>
          <Form.Item name="password" label="密码" rules={[{ required: true, message: '请输入密码' }]}>
            <Input.Password placeholder="请输入密码" disabled={loading} onPressEnter={() => void submit()} />
          </Form.Item>
        </Form>
      )}
    </Modal>
  );
}
