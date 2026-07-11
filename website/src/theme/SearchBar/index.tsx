import {useCallback, useEffect, useRef, useState} from 'react';

import {translate} from '@docusaurus/Translate';
import {useHistory} from '@docusaurus/router';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import clsx from 'clsx';

import styles from './styles.module.css';

type PagefindSubResult = {
  title: string;
  url: string;
  excerpt: string;
};

type PagefindResultData = {
  url: string;
  meta: {title: string};
  excerpt: string;
  sub_results: PagefindSubResult[];
};

type PagefindResult = {
  data: () => Promise<PagefindResultData>;
};

type Pagefind = {
  init: () => Promise<void>;
  debouncedSearch: (query: string) => Promise<{results: PagefindResult[]} | null>;
};

declare global {
  interface Window {
    aidememoLoadPagefind?: (url: string) => Promise<Pagefind>;
  }
}

type SearchResult = {
  pageTitle: string;
  title: string;
  url: string;
  excerpt: string;
  firstInGroup: boolean;
};

function flattenResults(pages: PagefindResultData[]): SearchResult[] {
  const rows: SearchResult[] = [];
  for (const page of pages) {
    if (page.sub_results?.length) {
      page.sub_results.forEach((result, index) => {
        rows.push({
          pageTitle: page.meta.title,
          title: result.title,
          url: result.url,
          excerpt: result.excerpt,
          firstInGroup: index === 0,
        });
      });
    } else {
      rows.push({
        pageTitle: page.meta.title,
        title: page.meta.title,
        url: page.url,
        excerpt: page.excerpt,
        firstInGroup: true,
      });
    }
  }
  return rows.slice(0, 12);
}

export default function SearchBar(): JSX.Element {
  const {
    siteConfig: {baseUrl},
  } = useDocusaurusContext();
  const history = useHistory();
  const containerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const pagefindRef = useRef<Pagefind | null>(null);
  const requestRef = useRef(0);
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<SearchResult[]>([]);
  const [activeIndex, setActiveIndex] = useState(-1);
  const [status, setStatus] = useState<'idle' | 'loading' | 'ready' | 'empty' | 'error'>('idle');
  const isOpen = query.trim().length > 0 && status !== 'idle';

  const loadPagefind = useCallback(async (): Promise<Pagefind> => {
    if (pagefindRef.current) {
      return pagefindRef.current;
    }
    if (!window.aidememoLoadPagefind) {
      await new Promise<void>((resolve, reject) => {
        const existing = document.querySelector<HTMLScriptElement>('script[data-pagefind-loader]');
        const onLoad = (): void => resolve();
        const onError = (): void => reject(new Error('Unable to load the Pagefind module loader.'));
        if (existing) {
          existing.addEventListener('load', onLoad, {once: true});
          existing.addEventListener('error', onError, {once: true});
          return;
        }
        const script = document.createElement('script');
        script.type = 'module';
        script.src = `${baseUrl}pagefind-loader.js`;
        script.dataset.pagefindLoader = 'true';
        script.addEventListener('load', onLoad, {once: true});
        script.addEventListener('error', onError, {once: true});
        document.head.appendChild(script);
      });
    }
    if (!window.aidememoLoadPagefind) {
      throw new Error('Pagefind module loader did not initialize.');
    }
    const pagefind = await window.aidememoLoadPagefind(`${baseUrl}pagefind/pagefind.js`);
    await pagefind.init();
    pagefindRef.current = pagefind;
    return pagefind;
  }, [baseUrl]);

  const selectResult = useCallback(
    (url: string): void => {
      setQuery('');
      setResults([]);
      setStatus('idle');
      setActiveIndex(-1);
      history.push(url.replace(/(?:index)?\.html(?=$|#)/, ''));
    },
    [history],
  );

  const search = useCallback(
    async (value: string): Promise<void> => {
      setQuery(value);
      const normalized = value.trim();
      const request = ++requestRef.current;
      if (!normalized) {
        setResults([]);
        setStatus('idle');
        setActiveIndex(-1);
        return;
      }

      setStatus('loading');
      try {
        const pagefind = await loadPagefind();
        const response = await pagefind.debouncedSearch(normalized);
        if (request !== requestRef.current || !response) {
          return;
        }
        const pages = await Promise.all(response.results.slice(0, 8).map((result) => result.data()));
        const nextResults = flattenResults(pages);
        setResults(nextResults);
        setActiveIndex(nextResults.length ? 0 : -1);
        setStatus(nextResults.length ? 'ready' : 'empty');
      } catch {
        if (request === requestRef.current) {
          setResults([]);
          setActiveIndex(-1);
          setStatus('error');
        }
      }
    },
    [loadPagefind],
  );

  useEffect(() => {
    function onPointerDown(event: MouseEvent): void {
      if (containerRef.current && !containerRef.current.contains(event.target as Node)) {
        setQuery('');
        setResults([]);
        setStatus('idle');
        setActiveIndex(-1);
      }
    }
    document.addEventListener('mousedown', onPointerDown);
    return () => document.removeEventListener('mousedown', onPointerDown);
  }, []);

  useEffect(() => {
    function onShortcut(event: KeyboardEvent): void {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        inputRef.current?.focus();
      }
    }
    document.addEventListener('keydown', onShortcut);
    return () => document.removeEventListener('keydown', onShortcut);
  }, []);

  const placeholder = translate({
    id: 'search.placeholder',
    message: 'Search docs',
    description: 'Placeholder for the documentation search input.',
  });
  const statusMessage =
    status === 'loading'
      ? translate({id: 'search.loading', message: 'Searching…'})
      : status === 'empty'
        ? translate({id: 'search.empty', message: 'No results found.'})
        : status === 'error'
          ? translate({id: 'search.error', message: 'Search is unavailable.'})
          : '';

  return (
    <div className={styles.container} ref={containerRef}>
      <input
        aria-activedescendant={activeIndex >= 0 ? `pagefind-result-${activeIndex}` : undefined}
        aria-autocomplete="list"
        aria-controls="pagefind-results"
        aria-expanded={isOpen}
        aria-label={placeholder}
        className={clsx('navbar__search-input', styles.input)}
        onChange={(event) => void search(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            setQuery('');
            setResults([]);
            setStatus('idle');
            setActiveIndex(-1);
            inputRef.current?.blur();
          } else if (event.key === 'ArrowDown' && results.length) {
            event.preventDefault();
            setActiveIndex((current) => (current + 1) % results.length);
          } else if (event.key === 'ArrowUp' && results.length) {
            event.preventDefault();
            setActiveIndex((current) => (current <= 0 ? results.length - 1 : current - 1));
          } else if (event.key === 'Enter' && activeIndex >= 0) {
            event.preventDefault();
            selectResult(results[activeIndex].url);
          }
        }}
        placeholder={placeholder}
        ref={inputRef}
        role="combobox"
        type="search"
        value={query}
      />

      {isOpen && (
        <div className={styles.panel}>
          {status === 'ready' ? (
            <ul aria-label={placeholder} className={styles.results} id="pagefind-results" role="listbox">
              {results.map((result, index) => (
                <li key={`${result.url}-${index}`} role="presentation">
                  {result.firstInGroup && result.title !== result.pageTitle && (
                    <p className={styles.pageTitle}>{result.pageTitle}</p>
                  )}
                  <a
                    aria-selected={index === activeIndex}
                    className={clsx(styles.result, index === activeIndex && styles.resultActive)}
                    href={result.url}
                    id={`pagefind-result-${index}`}
                    onClick={(event) => {
                      event.preventDefault();
                      selectResult(result.url);
                    }}
                    onMouseEnter={() => setActiveIndex(index)}
                    role="option"
                  >
                    <strong>{result.title}</strong>
                    <span dangerouslySetInnerHTML={{__html: result.excerpt}} />
                  </a>
                </li>
              ))}
            </ul>
          ) : (
            <p className={styles.status} role="status">
              {statusMessage}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
