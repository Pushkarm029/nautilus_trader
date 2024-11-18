// -------------------------------------------------------------------------------------------------
//  Copyright (C) 2015-2024 Nautech Systems Pty Ltd. All rights reserved.
//  https://nautechsystems.io
//
//  Licensed under the GNU Lesser General Public License Version 3.0 (the "License");
//  You may not use this file except in compliance with the License.
//  You may obtain a copy of the License at https://www.gnu.org/licenses/lgpl-3.0.en.html
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.
// -------------------------------------------------------------------------------------------------

//! Provides account management functionality.

// Under development
#![allow(dead_code)]
#![allow(unused_variables)]

use std::{cell::RefCell, rc::Rc};

use nautilus_common::{cache::Cache, clock::Clock};
use nautilus_core::{ffi::uuid::uuid4_new, nanos::UnixNanos};
use nautilus_model::{
    accounts::{any::AccountAny, base::Account, cash::CashAccount, margin::MarginAccount},
    enums::{AccountType, OrderSide, OrderSideSpecified, PriceType},
    events::{account::state::AccountState, order::OrderFilled},
    instruments::any::InstrumentAny,
    orders::any::OrderAny,
    position::Position,
    types::{balance::AccountBalance, money::Money},
};
use rust_decimal::{prelude::ToPrimitive, Decimal};
pub struct AccountsManager {
    clock: Rc<RefCell<dyn Clock>>,
    cache: Rc<RefCell<Cache>>,
}

impl AccountsManager {
    #[must_use]
    pub fn update_balances(
        &self,
        account: AccountAny,
        instrument: InstrumentAny,
        fill: OrderFilled,
    ) -> AccountState {
        let cache = self.cache.borrow();
        let position_id = if let Some(position_id) = fill.position_id {
            position_id
        } else {
            let positions_open = cache.positions_open(None, Some(&fill.instrument_id), None, None);

            // TODO: error handling
            positions_open
                .first()
                .unwrap_or_else(|| {
                    log::error!("List of Positions is empty");
                    panic!("List of Positions is empty")
                })
                .id
        };

        let position = cache.position(&position_id);

        let pnls = account.calculate_pnls(instrument, fill, position.cloned());

        // Calculate final PnL including commissions
        match account.base_currency() {
            Some(base_currency) => {
                let pnl = pnls.map_or_else(
                    |_| Money::new(0.0, base_currency),
                    |pnl_list| {
                        pnl_list
                            .first()
                            .copied()
                            .unwrap_or_else(|| Money::new(0.0, base_currency))
                    },
                );

                self.update_balance_single_currency(account.clone(), &fill, pnl);
            }
            None => {
                if let Ok(pnl_list) = pnls {
                    self.update_balance_multi_currency(account.clone(), fill, &pnl_list);
                }
            }
        }

        // Generate and return account state
        self.generate_account_state(account, fill.ts_event)
    }

    #[must_use]
    pub fn update_orders(
        &self,
        account: AccountAny,
        instrument: InstrumentAny,
        orders_open: &[OrderAny],
        ts_event: UnixNanos,
    ) -> Option<AccountState> {
        // todo!()
        match account {
            AccountAny::Cash(mut cash_account) => {
                self.update_balance_locked(&mut cash_account, instrument, orders_open, ts_event)
            }
            AccountAny::Margin(mut margin_account) => {
                self.update_margin_init(&mut margin_account, instrument, orders_open, ts_event)
            }
        }
    }

    // TODO: too many clones inside this
    #[must_use]
    pub fn update_positions(
        &self,
        account: &mut MarginAccount,
        instrument: InstrumentAny,
        positions: &[Position],
        ts_event: UnixNanos,
    ) -> Option<AccountState> {
        // Initialize variables
        let mut total_margin_maint = Decimal::ZERO;
        let mut base_xrate = Decimal::ZERO;
        let mut currency = instrument.settlement_currency();

        // Process each position
        for position in positions {
            // Verify position is for correct instrument
            assert_eq!(
                position.instrument_id,
                instrument.id(),
                "Position not for instrument {}",
                instrument.id()
            );

            // Skip closed positions
            if !position.is_open() {
                continue;
            }

            // TODO: Can be simplified after implementing Instrument trait for InstrumentAny
            let margin_maint = match instrument {
                InstrumentAny::Betting(i) => account.calculate_maintenance_margin(
                    i,
                    position.quantity,
                    instrument.make_price(position.avg_px_open),
                    None,
                ),
                InstrumentAny::BinaryOption(i) => account.calculate_maintenance_margin(
                    i,
                    position.quantity,
                    instrument.make_price(position.avg_px_open),
                    None,
                ),
                InstrumentAny::CryptoFuture(i) => account.calculate_maintenance_margin(
                    i,
                    position.quantity,
                    instrument.make_price(position.avg_px_open),
                    None,
                ),
                InstrumentAny::CryptoPerpetual(i) => account.calculate_maintenance_margin(
                    i,
                    position.quantity,
                    instrument.make_price(position.avg_px_open),
                    None,
                ),
                InstrumentAny::CurrencyPair(i) => account.calculate_maintenance_margin(
                    i,
                    position.quantity,
                    instrument.make_price(position.avg_px_open),
                    None,
                ),
                InstrumentAny::Equity(i) => account.calculate_maintenance_margin(
                    i,
                    position.quantity,
                    instrument.make_price(position.avg_px_open),
                    None,
                ),
                InstrumentAny::FuturesContract(i) => account.calculate_maintenance_margin(
                    i,
                    position.quantity,
                    instrument.make_price(position.avg_px_open),
                    None,
                ),
                InstrumentAny::FuturesSpread(i) => account.calculate_maintenance_margin(
                    i,
                    position.quantity,
                    instrument.make_price(position.avg_px_open),
                    None,
                ),
                InstrumentAny::OptionsContract(i) => account.calculate_maintenance_margin(
                    i,
                    position.quantity,
                    instrument.make_price(position.avg_px_open),
                    None,
                ),
                InstrumentAny::OptionsSpread(i) => account.calculate_maintenance_margin(
                    i,
                    position.quantity,
                    instrument.make_price(position.avg_px_open),
                    None,
                ),
            };

            let mut margin_maint = margin_maint.as_decimal();

            // Handle base currency conversion if needed
            if let Some(base_currency) = account.base_currency {
                if base_xrate.is_zero() {
                    // Cache base currency and calculate exchange rate
                    currency = base_currency;
                    base_xrate = self.calculate_xrate_to_base(
                        AccountAny::Margin(account.clone()),
                        instrument.clone(),
                        position.entry.as_specified(),
                    );

                    if base_xrate == Decimal::ZERO {
                        log::debug!("Cannot calculate maintenance (position) margin: insufficient data for {}/{}", instrument.settlement_currency(), base_currency);
                        return None;
                    }
                }

                // Apply base exchange rate
                margin_maint = (margin_maint * base_xrate).round_dp(currency.precision.into());
            }

            // Increment total maintenance margin
            total_margin_maint += margin_maint;
        }

        // Create Money object for margin maintenance
        let margin_maint_money = Money::new(total_margin_maint.to_f64()?, currency);

        // Update account margin maintenance
        account.update_maintenance_margin(instrument.id(), margin_maint_money);

        // Log the update
        log::info!(
            "{} margin_maint={}",
            instrument.id(),
            margin_maint_money.to_string()
        );

        // Generate and return account state
        Some(self.generate_account_state(AccountAny::Margin(account.clone()), ts_event))
    }

    // TODO: improve error handling
    fn update_balance_locked(
        &self,
        account: &mut CashAccount,
        instrument: InstrumentAny,
        orders_open: &[OrderAny],
        ts_event: UnixNanos,
    ) -> Option<AccountState> {
        if orders_open.is_empty() {
            // TODO: fix
            // account.clear_balance_locked(&instrument.id());
            return Some(self.generate_account_state(AccountAny::Cash(account.clone()), ts_event));
        }

        // Initialize variables
        let mut total_locked = Decimal::ZERO;
        let mut base_xrate = Decimal::ZERO;

        let mut currency = instrument.settlement_currency();

        // Process each open order
        for order in orders_open {
            // Verify order is for correct instrument
            assert_eq!(
                order.instrument_id(),
                instrument.id(),
                "Order not for instrument {}",
                instrument.id()
            );
            assert!(order.is_open(), "Order is not open");

            // Skip orders without price or trigger price
            if order.price().is_none() && order.trigger_price().is_none() {
                continue;
            }

            // Calculate locked balance for this order
            let price = if order.price().is_some() {
                order.price()
            } else {
                order.trigger_price()
            };

            let mut locked = account
                .calculate_balance_locked(
                    instrument.clone(),
                    order.order_side(),
                    order.quantity(),
                    price?,
                    None,
                )
                .unwrap()
                .as_decimal();

            // Handle base currency conversion if needed
            if let Some(base_curr) = account.base_currency() {
                if base_xrate.is_zero() {
                    // Cache base currency and calculate exchange rate
                    currency = base_curr;
                    base_xrate = self.calculate_xrate_to_base(
                        AccountAny::Cash(account.clone()),
                        instrument.clone(),
                        order.order_side_specified(),
                    );
                }

                // Apply base exchange rate and round to currency precision
                locked = (locked * base_xrate).round_dp(u32::from(currency.precision));
            }

            // Add to total locked amount
            total_locked += locked;
        }

        // Create Money object for locked balance
        let locked_money = Money::new(total_locked.to_f64()?, currency);

        // Update account locked balance
        // account.update_balance_locked(&instrument.id(), locked_money.clone());

        // Log the update
        log::info!(
            "{} balance_locked={}",
            instrument.id(),
            locked_money.to_string()
        );

        // Generate and return account state
        Some(self.generate_account_state(AccountAny::Cash(account.clone()), ts_event))
    }

    fn update_margin_init(
        &self,
        account: &mut MarginAccount,
        instrument: InstrumentAny,
        orders_open: &[OrderAny],
        ts_event: UnixNanos,
    ) -> Option<AccountState> {
        // Initialize variables
        let mut total_margin_init = Decimal::ZERO;
        let mut base_xrate = Decimal::ZERO;
        let mut currency = instrument.settlement_currency();

        // Process each order
        for order in orders_open {
            assert_eq!(
                order.instrument_id(),
                instrument.id(),
                "Order not for instrument {}",
                instrument.id()
            );

            // Skip if not open or no price/trigger price
            if !order.is_open() || (order.price().is_none() && order.trigger_price().is_none()) {
                continue;
            }

            // Calculate initial margin based on instrument type
            let price = if order.price().is_some() {
                order.price()
            } else {
                order.trigger_price()
            };

            let margin_init = match instrument {
                InstrumentAny::Betting(i) => {
                    account.calculate_initial_margin(i, order.quantity(), price?, None)
                }
                InstrumentAny::BinaryOption(i) => {
                    account.calculate_initial_margin(i, order.quantity(), price?, None)
                }
                InstrumentAny::CryptoFuture(i) => {
                    account.calculate_initial_margin(i, order.quantity(), price?, None)
                }
                InstrumentAny::CryptoPerpetual(i) => {
                    account.calculate_initial_margin(i, order.quantity(), price?, None)
                }
                InstrumentAny::CurrencyPair(i) => {
                    account.calculate_initial_margin(i, order.quantity(), price?, None)
                }
                InstrumentAny::Equity(i) => {
                    account.calculate_initial_margin(i, order.quantity(), price?, None)
                }
                InstrumentAny::FuturesContract(i) => {
                    account.calculate_initial_margin(i, order.quantity(), price?, None)
                }
                InstrumentAny::FuturesSpread(i) => {
                    account.calculate_initial_margin(i, order.quantity(), price?, None)
                }
                InstrumentAny::OptionsContract(i) => {
                    account.calculate_initial_margin(i, order.quantity(), price?, None)
                }
                InstrumentAny::OptionsSpread(i) => {
                    account.calculate_initial_margin(i, order.quantity(), price?, None)
                }
            };

            let mut margin_init = margin_init.as_decimal();

            // Handle base currency conversion if needed
            if let Some(base_currency) = account.base_currency {
                if base_xrate.is_zero() {
                    // Cache base currency and calculate exchange rate
                    currency = base_currency;
                    base_xrate = self.calculate_xrate_to_base(
                        AccountAny::Margin(account.clone()),
                        instrument.clone(),
                        order.order_side_specified(),
                    );

                    if base_xrate == Decimal::ZERO {
                        log::debug!(
                            "Cannot calculate initial margin: insufficient data for {}/{}",
                            instrument.settlement_currency(),
                            base_currency
                        );
                        continue;
                    }
                }

                // Apply base exchange rate
                margin_init = (margin_init * base_xrate).round_dp(currency.precision.into());
            }

            // Increment total initial margin
            total_margin_init += margin_init;
        }

        // Create Money object for margin init
        let money = Money::new(total_margin_init.to_f64().unwrap_or(0.0), currency);
        let margin_init_money = {
            // Update account initial margin
            account.update_initial_margin(instrument.id(), money);
            money
        };

        // Log the update
        log::info!(
            "{} margin_init={}",
            instrument.id(),
            margin_init_money.to_string()
        );

        // Generate and return account state
        Some(self.generate_account_state(AccountAny::Margin(account.clone()), ts_event))
    }

    fn update_balance_single_currency(&self, account: AccountAny, fill: &OrderFilled, pnl: Money) {
        let base_currency = account.base_currency().unwrap();
        let mut final_pnl = pnl;

        if let Some(commission) = &fill.commission {
            let commission = if commission.currency != base_currency {
                let xrate = self.cache.borrow().get_xrate(
                    fill.instrument_id.venue,
                    commission.currency,
                    base_currency,
                    if fill.order_side == OrderSide::Sell {
                        PriceType::Bid
                    } else {
                        PriceType::Ask
                    },
                );

                if xrate == Decimal::ZERO {
                    log::error!(
                        "Cannot calculate account state: insufficient data for {}/{:?}",
                        pnl.currency,
                        account.base_currency()
                    );
                }

                Money::new(xrate.to_f64().expect("msg"), commission.currency)
            } else {
                *commission
            };

            if pnl.currency != base_currency {
                let xrate: Decimal = self.cache.borrow().get_xrate(
                    fill.instrument_id.venue,
                    pnl.currency,
                    base_currency,
                    if fill.order_side == OrderSide::Sell {
                        PriceType::Bid
                    } else {
                        PriceType::Ask
                    },
                );

                if xrate == Decimal::ZERO {
                    log::error!(
                        "Cannot calculate account state: insufficient data for {}/{:?}",
                        pnl.currency,
                        account.base_currency()
                    );
                }

                final_pnl = Money::new(
                    (pnl.as_decimal() * xrate).to_f64().unwrap_or(0.0),
                    base_currency,
                );
            }

            final_pnl -= commission;
            if final_pnl.is_zero() {
                return; // Nothing to Adjust
            }

            // // Get current balance
            // let balance = match account.balance(None) {
            //     Some(b) => b,
            //     None => {
            //         log::error!(
            //             "Cannot complete transaction: no balance for {}",
            //             final_pnl.currency()
            //         );
            //         return;
            //     }
            // };

            // // Calculate new balance
            // let new_balance = AccountBalance::new(
            //     balance.total().add(&final_pnl),
            //     balance.locked().clone(),
            //     balance.free().add(&final_pnl),
            // );

            // // Update account with new balances and commission
            // match account {
            //     AccountAny::Cash(mut cash_account) => {
            //         cash_account.update_balances(vec![new_balance]);
            //         cash_account.update_commissions(commission);
            //     }
            //     AccountAny::Margin(mut margin_account) => {
            //         margin_account.update_balances(vec![new_balance]);
            //         margin_account.update_commissions(commission);
            //     }
            // }
            todo!("")
        } else {
            // No commission to process, just update the balance with PnL

            // Get current balance
            // let balance = match account.balance(None) {
            //     Some(b) => b,
            //     None => {
            //         log::error!(
            //             "Cannot complete transaction: no balance for {}",
            //             final_pnl.currency
            //         );
            //         return;
            //     }
            // };

            // // Calculate new balance
            // let new_balance = AccountBalance::new(
            //     balance.total().add(&final_pnl),
            //     balance.locked().clone(),
            //     balance.free().add(&final_pnl),
            // );

            // // Update account with new balance only
            // match account {
            //     AccountAny::Cash(mut cash_account) => {
            //         cash_account.update_balances(vec![new_balance]);
            //     }
            //     AccountAny::Margin(mut margin_account) => {
            //         margin_account.update_balances(vec![new_balance]);
            //     }
            // }
            todo!("")
        }
    }

    fn update_balance_multi_currency(
        &self,
        account: AccountAny,
        fill: OrderFilled,
        pnls: &[Money],
    ) {
        // let balances =

        let commission = fill.commission;
        let balance: Option<AccountBalance> = None;
        let new_balance: Option<AccountBalance> = None;
        let apply_commission = if let Some(commission) = commission {
            commission.as_decimal() != Decimal::ZERO
        } else {
            false
        };

        // for pnl in pnls {
        //     if apply_commission && pnl.currency == commission.unwrap().currency {}

        //     if pnl.is_zero() {
        //         continue; // No adjustment
        //     }

        //     let currency = pnl.currency;
        //     // let balance = account.balances_locked()
        // }
        todo!()
    }

    fn generate_account_state(&self, account: AccountAny, ts_event: UnixNanos) -> AccountState {
        match account {
            AccountAny::Cash(cash_account) => AccountState::new(
                cash_account.id,
                AccountType::Cash,
                cash_account.balances.clone().into_values().collect(),
                vec![],
                false,
                uuid4_new(),
                ts_event,
                self.clock.borrow().timestamp_ns(),
                cash_account.base_currency(),
            ),
            AccountAny::Margin(margin_account) => AccountState::new(
                margin_account.id,
                AccountType::Cash,
                vec![],
                margin_account.margins.clone().into_values().collect(),
                false,
                uuid4_new(),
                ts_event,
                self.clock.borrow().timestamp_ns(),
                margin_account.base_currency(),
            ),
        }
    }

    fn calculate_xrate_to_base(
        &self,
        account: AccountAny,
        instrument: InstrumentAny,
        side: OrderSideSpecified,
    ) -> Decimal {
        match account.base_currency() {
            None => Decimal::ONE,
            Some(base_curr) => self.cache.borrow().get_xrate(
                instrument.id().venue,
                instrument.settlement_currency(),
                base_curr,
                match side {
                    OrderSideSpecified::Sell => PriceType::Bid,
                    OrderSideSpecified::Buy => PriceType::Ask,
                },
            ),
        }
    }
}
