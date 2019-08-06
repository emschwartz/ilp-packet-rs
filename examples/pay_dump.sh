#!/bin/bash

# who do we send to?
if [[ $1 == bob ]]
then
    PORT=7770
    RECEIVER=8770
    AUTH=in_alice
elif [[ $1 == bob ]]
then
    PORT=8770
    RECEIVER=7770
    AUTH=in_bob
elif [[ $1 == charlie ]]
then # alice to charlie
    PORT=7770
    RECEIVER=9770
    AUTH=in_alice
fi

if [ ! -z "$2"  ]
then
    set -x
    curl localhost:$PORT/pay \
        -d "{ \"receiver\" : \"http://localhost:$RECEIVER\", \"source_amount\": $2  }" \
        -H "Authorization: Bearer $AUTH" -H "Content-Type: application/json"
    set +x
fi

printf "\n----\n"

echo "Bob's balance on Alice's store"
curl localhost:7770/accounts/1/balance -H "Authorization: Bearer bob"

printf "\n----\n"

echo "Alice's balance on Bob's store"
curl localhost:8770/accounts/1/balance -H "Authorization: Bearer alice"
