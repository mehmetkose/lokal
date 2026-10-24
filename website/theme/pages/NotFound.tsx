// The 404 page body. The Layout supplies the document, header, <main>, footer and
// the head tags (title "Page not found", noindex).
import { LogoMark } from '../components/layout/LogoMark';
import { Button } from '../components/ui/Button';
import { TornEdge } from '../components/ui/TornEdge';
import './NotFound.css';

export function NotFound() {
  return (
    <section className="not-found bg-green">
      <div className="container not-found__inner" data-reveal="">
        <p className="not-found__code" aria-hidden="true">
          4<LogoMark className="not-found__mark" />4
        </p>
        <div className="not-found__text">
          <h1 className="h3 c-white">No route for this name</h1>
          <p className="not-found__desc">
            This page isn’t configured. It may have moved when the docs were reorganised.
          </p>
        </div>
        <div className="not-found__actions">
          <Button href="/" variant="yellow">
            Go back home
          </Button>
          <Button href="/docs" variant="orange">
            Read the docs
          </Button>
        </div>
      </div>
      <TornEdge position="bottom" color="green" />
    </section>
  );
}
