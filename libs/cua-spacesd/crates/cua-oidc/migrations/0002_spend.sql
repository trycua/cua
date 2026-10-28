-- SPDX-License-Identifier: FSL-1.1-MIT
-- What each device spent on a connection in a calendar month (UTC), as it
-- reported it (its running total: a new report replaces the old one). A
-- connection's month spend is the sum over the account's devices.

CREATE TABLE oidc_spend (
    account    text             NOT NULL,
    connection text             NOT NULL,
    device     text             NOT NULL,
    month      text             NOT NULL CHECK (month ~ '^[0-9]{4}-[0-9]{2}$'),
    usd        double precision NOT NULL CHECK (usd >= 0),
    updated_at bigint           NOT NULL,
    PRIMARY KEY (account, connection, month, device)
);
