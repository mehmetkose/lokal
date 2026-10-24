import { usePage } from '@rspress/core/runtime';
import { useEffect, useRef, useState } from 'react';
import { cx } from '../../lib/cx';
import { docsMenu, docsMenuTip, links, mainNav, mobileNav } from '../../data/site';
import { Button } from '../ui/Button';
import { Icon } from '../ui/Icon';
import { Logo } from './Logo';
import { withBase } from '../../lib/url';
import './Header.css';

const linkHrefs = mainNav.map((l) => l.href);

export function Header() {
  const { page } = usePage();
  // Astro's file-format build prerendered the home page as /index.html (path '/index'), so the
  // mobile "Home" link was never marked current there. Keep that.
  const routePath = page.routePath === '/' ? '/index.html' : page.routePath;
  const path = routePath.replace(/(\.html|\/)$/, '') || '/';
  // A docs page highlights its own header link if it has one, otherwise "Docs".
  const isActive = (href: string) => path === href;
  const docsActive = path.startsWith('/docs') && !linkHrefs.includes(path);

  // Desktop "Docs" dropdown: a mouse opens it by hovering, a click or tap toggles it.
  // Hover is mouse-only, because a tap also fires enter events: it would open the
  // panel just before its click closed it again.
  const [dropdownOpen, setDropdownOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const closeTimer = useRef<number | undefined>(undefined);
  // True while the panel is open because of hover alone. The first click then keeps
  // it open: the mouse is already on "Docs", so a toggle would close it.
  const openedByHover = useRef(false);

  // Mobile menu
  const [menuOpen, setMenuOpen] = useState(false);

  useEffect(() => {
    const onClick = (e: MouseEvent) => {
      if (!dropdownRef.current?.contains(e.target as Node)) setDropdownOpen(false);
    };
    const onKeydown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setDropdownOpen(false);
    };
    document.addEventListener('click', onClick);
    document.addEventListener('keydown', onKeydown);
    return () => {
      document.removeEventListener('click', onClick);
      document.removeEventListener('keydown', onKeydown);
      window.clearTimeout(closeTimer.current);
    };
  }, []);

  return (
    <header className={cx('header', menuOpen && 'menu-open')} data-header="">
      <div className="container header__bar">
        <Logo />

        <nav className="header__nav" aria-label="Main">
          <div
            className={cx('dropdown', dropdownOpen && 'is-open')}
            data-dropdown=""
            ref={dropdownRef}
            onPointerEnter={(e) => {
              if (e.pointerType !== 'mouse') return;
              window.clearTimeout(closeTimer.current);
              if (!dropdownOpen) openedByHover.current = true;
              setDropdownOpen(true);
            }}
            onPointerLeave={(e) => {
              if (e.pointerType !== 'mouse') return;
              window.clearTimeout(closeTimer.current);
              closeTimer.current = window.setTimeout(() => setDropdownOpen(false), 150);
            }}
          >
            <button
              type="button"
              className={cx('header__link', 'dropdown__trigger', docsActive && 'is-active')}
              aria-expanded={dropdownOpen}
              aria-controls="docs-menu"
              data-dropdown-trigger=""
              onClick={() => {
                const keepOpen = openedByHover.current;
                openedByHover.current = false;
                setDropdownOpen((open) => keepOpen || !open);
              }}
            >
              Docs <Icon name="chevron-down" size={16} className="dropdown__chevron" />
            </button>
            <div className="dropdown__panel" id="docs-menu" data-dropdown-panel="">
              {docsMenu.map((col) => (
                <div className={cx('dropdown__group', col.featured && 'dropdown__group--featured')} key={col.label}>
                  <p className="dropdown__heading">{col.label}</p>
                  <ul className="dropdown__col">
                    {col.items.map((item) => (
                      <li key={item.href}>
                        <a
                          href={withBase(item.href)}
                          className="dropdown__link"
                          aria-current={isActive(item.href) ? 'page' : undefined}
                        >
                          <span className="dropdown__icon">
                            <Icon name={item.icon} size={col.featured ? 18 : 17} />
                          </span>
                          <span className="dropdown__text">
                            <span className="dropdown__title">{item.label}</span>
                            {item.desc && <span className="dropdown__desc">{item.desc}</span>}
                          </span>
                        </a>
                      </li>
                    ))}
                  </ul>
                </div>
              ))}
              <a href={withBase(docsMenuTip.href)} className="dropdown__tip">
                <code className="dropdown__cmd">
                  <span aria-hidden="true">$ </span>
                  {docsMenuTip.cmd}
                </code>
                <span className="dropdown__tip-text">{docsMenuTip.text}</span>
                <Icon name="arrow-right" size={16} className="dropdown__tip-arrow" />
              </a>
            </div>
          </div>
          {mainNav.map((item) => (
            <a
              key={item.href}
              href={withBase(item.href)}
              className={cx('header__link', isActive(item.href) && 'is-active')}
              aria-current={isActive(item.href) ? 'page' : undefined}
            >
              {item.label}
            </a>
          ))}
        </nav>

        <div className="header__actions">
          <a href={links.repo} className="header__github" aria-label="lokal on GitHub">
            <Icon name="github" size={22} />
            <span>GitHub</span>
          </a>
          <Button href="/docs/installation" variant="yellow" className="header__cta">
            Install
          </Button>
        </div>

        <button
          className="burger"
          aria-label={menuOpen ? 'Close menu' : 'Open menu'}
          aria-expanded={menuOpen}
          aria-controls="mobile-menu"
          data-burger=""
          onClick={() => setMenuOpen((open) => !open)}
        >
          <Icon name="menu" size={24} className="burger__open" />
          <Icon name="close" size={24} className="burger__close" />
        </button>
      </div>

      <div className="mobile-menu" id="mobile-menu" data-mobile-menu="">
        <ul className="container mobile-menu__list">
          {mobileNav.map((item) => (
            <li key={item.href}>
              <a
                href={withBase(item.href)}
                className="mobile-menu__link"
                aria-current={isActive(item.href) ? 'page' : undefined}
              >
                {item.label}
              </a>
            </li>
          ))}
          <li className="mobile-menu__extra">
            <a href={links.repo} className="mobile-menu__link mobile-menu__github">
              <Icon name="github" size={20} /> GitHub
            </a>
            <Button href="/docs/installation" variant="yellow" full>
              Install lokal
            </Button>
          </li>
        </ul>
      </div>
    </header>
  );
}
