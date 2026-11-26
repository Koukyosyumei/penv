use crate::address_range::{AddressRange, MinimalRanges};
use crate::symbol::{Symbol, SymbolBinding, SymbolType};
use std::cmp::Reverse;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, Mutex, Weak};

type SymbolKey = (String, u64, u64, SymbolType, SymbolBinding);
type InternedSymbols = HashMap<SymbolKey, Weak<Symbol>>;
static SYMBOLS: LazyLock<Mutex<InternedSymbols>> = LazyLock::new(Mutex::default);

#[derive(Default)]
pub struct SymbolLookup {
    symbols: Vec<Arc<Symbol>>,
}

impl SymbolLookup {
    pub fn insert_symbols(&mut self, symbols: impl IntoIterator<Item = Symbol>) {
        let symbols: Vec<_> = symbols
            .into_iter()
            .filter(|symbol| !symbol.name.is_empty())
            .collect();
        let mut interned = SYMBOLS.lock().unwrap();
        interned.retain(|_, symbol| symbol.strong_count() != 0);
        let incoming: Vec<_> = symbols
            .into_iter()
            .map(|symbol| Self::intern(&mut interned, symbol))
            .collect();

        let mut coverage = MinimalRanges::default();
        let mut split_points = BTreeSet::new();
        for symbol in &incoming {
            if symbol.start() < symbol.end() {
                // Exclusive symbol ends leave u64::MAX uncovered.
                coverage
                    .add(AddressRange::to(symbol.start(), symbol.end() - 1).unwrap())
                    .unwrap();
            } else {
                split_points.insert(symbol.start());
            }
        }
        let mut merged = Vec::with_capacity(self.symbols.len() + incoming.len());
        // When clipping collapses nested symbols to the same extent, keep
        // the symbol with the more specific original start first.
        self.symbols.sort_by_key(|symbol| Reverse(symbol.start()));
        for symbol in self.symbols.drain(..) {
            if symbol.start() == symbol.end() {
                if !coverage
                    .ranges()
                    .iter()
                    .any(|range| range.contains(symbol.start()))
                {
                    merged.push(symbol);
                }
                continue;
            }
            let original = AddressRange::to(symbol.start(), symbol.end() - 1).unwrap();
            let mut uncovered = vec![original];
            for &replacement in coverage.ranges() {
                uncovered = uncovered
                    .into_iter()
                    .flat_map(|range| range.subtract(replacement))
                    .collect();
            }
            for range in uncovered {
                let mut start = range.start;
                for &point in split_points.range((
                    std::ops::Bound::Excluded(start),
                    std::ops::Bound::Included(range.end),
                )) {
                    merged.push(Self::fragment(&mut interned, &symbol, start, point));
                    start = point;
                }
                merged.push(Self::fragment(&mut interned, &symbol, start, range.end + 1));
            }
        }
        merged.extend(incoming);
        merged.sort_by_key(|symbol| symbol.start());
        self.symbols = merged;
    }

    fn fragment(
        interned: &mut InternedSymbols,
        symbol: &Arc<Symbol>,
        start: u64,
        end: u64,
    ) -> Arc<Symbol> {
        if start == symbol.start() && end == symbol.end() {
            Arc::clone(symbol)
        } else {
            Self::intern(interned, symbol.with_bounds(start, end))
        }
    }

    fn intern(interned: &mut InternedSymbols, symbol: Symbol) -> Arc<Symbol> {
        let key = (
            symbol.name.clone(),
            symbol.start(),
            symbol.end(),
            symbol.kind,
            symbol.binding,
        );
        if let Some(existing) = interned.get(&key).and_then(Weak::upgrade) {
            return existing;
        }
        let symbol = Arc::new(symbol);
        interned.insert(key, Arc::downgrade(&symbol));
        symbol
    }

    pub fn by_name<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Arc<Symbol>> + 'a {
        self.symbols.iter().filter(move |symbol| {
            symbol.name == name
                || symbol
                    .name
                    .split_once('.')
                    .is_some_and(|(prefix, _)| prefix == name)
        })
    }

    pub fn by_address(&self, address: u64) -> Option<&Arc<Symbol>> {
        let priority = |symbol: &&Arc<Symbol>| {
            (
                Reverse(symbol.start()),
                symbol.end(),
                Reverse(symbol.kind),
                Reverse(symbol.binding),
            )
        };
        self.symbols
            .iter()
            .filter(|symbol| symbol.contains_address(address))
            .min_by_key(priority)
            .or_else(|| {
                if address > self.symbols.iter().map(|symbol| symbol.end()).max()? {
                    return None;
                }
                self.symbols
                    .iter()
                    .filter(|symbol| symbol.start() < address)
                    .min_by_key(priority)
                    .filter(|symbol| symbol.start() == symbol.end())
            })
    }
}
