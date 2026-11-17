import type { ComponentPropsWithRef } from 'react';

type ButtonProps = ComponentPropsWithRef<'button'> & {
  kind?: 'primary' | 'secondary' | 'quiet' | 'icon';
  size?: 'default' | 'small';
};

export function Button({ kind = 'secondary', size = 'default', type = 'button', className = '', ...props }: ButtonProps) {
  return <button {...props} type={type} className={`button button-${kind}${size === 'small' ? ' button-small' : ''}${className ? ` ${className}` : ''}`} />;
}
export function CloseIcon() {
  return <svg className="icon" viewBox="0 0 24 24" aria-hidden="true"><path d="m6 6 12 12M18 6 6 18" /></svg>;
}
export function CopyIcon() {
  return <svg className="icon" viewBox="0 0 24 24" aria-hidden="true"><rect x="8" y="8" width="12" height="13" rx="2" /><path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3" /></svg>;
}
