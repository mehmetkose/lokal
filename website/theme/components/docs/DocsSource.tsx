// Links a docs page back to the files in the lokal repo it was adapted from.
import { Fragment } from 'react';
import { sourceUrl } from '../../data/docs';
import { links } from '../../data/site';
import { Icon } from '../ui/Icon';
import './DocsSource.css';

type Props = {
  source: string[];
};

export function DocsSource({ source }: Props) {
  // Text after each file link: ", " / " and " between files, then the rest of
  // the sentence (one text node each, as in the Astro output).
  const after = (i: number) =>
    i < source.length - 2 ? ', ' : i === source.length - 2 ? ' and ' : ' in the lokal repository. ';

  return (
    <aside className="source">
      <Icon name="github" size={20} className="source__icon" />
      <p>
        {source.length > 0 ? (
          <>
            {'Adapted from '}
            {source.map((path, i) => (
              <Fragment key={path}>
                <a href={sourceUrl(path)} target="_blank" rel="noopener">
                  <code>{path}</code>
                </a>
                {after(i)}
              </Fragment>
            ))}
          </>
        ) : (
          'Something unclear or out of date? '
        )}
        <a href={links.issues} target="_blank" rel="noopener">
          Open an issue
        </a>
        .
      </p>
    </aside>
  );
}
