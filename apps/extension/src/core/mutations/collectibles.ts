// Copyright (c) Aptos
// SPDX-License-Identifier: Apache-2.0

import { AptosClient, TokenClient, RequestError } from 'aptos';
import { getIsValidMetadataStructure } from 'core/queries/collectibles';
import queryKeys from 'core/queries/queryKeys';
import { AptosAccountState } from 'core/types';
import { AptosNetwork } from 'core/utils/network';
import { useCallback } from 'react';
import { useMutation, useQueryClient } from 'react-query';

interface CreateTokenAndCollectionProps {
  account: AptosAccountState;
  collectionName?: string;
  description?: string;
  name?: string;
  nodeUrl: AptosNetwork;
  supply: number;
  uri?: string;
}

export const defaultRequestErrorAttributes = {
  config: {},
  headers: {},
  status: 400,
  statusText: 'Move abort',
};

const ERROR_CODES = Object.freeze({
  URI_GENERAL: 'URI is invalid',
  URI_METADATA_FORMAT: 'Wrong metadata format in URI',
} as const);

export interface RaiseForErrorProps {
  error?: string;
  vmStatus?: string
}

const raiseForError = ({
  error,
  vmStatus,
}: RaiseForErrorProps) => {
  if (error?.includes(ERROR_CODES.URI_METADATA_FORMAT)) {
    throw new RequestError(error, {
      data: {
        message: error,
      },
      ...defaultRequestErrorAttributes,
      statusText: error,
    });
  } else if (vmStatus?.includes('Move abort')) {
    throw new RequestError(vmStatus, {
      data: {
        message: vmStatus,
      },
      ...defaultRequestErrorAttributes,
    });
  }
};

export const createTokenAndCollection = async ({
  account,
  collectionName,
  description,
  name,
  nodeUrl,
  supply,
  uri,
}: CreateTokenAndCollectionProps): Promise<void> => {
  if (!account || !(collectionName && description && uri && name)) {
    return;
  }
  const isValidUri = await getIsValidMetadataStructure({ uri });
  raiseForError({
    error: (isValidUri)
      ? undefined
      : `${ERROR_CODES.URI_METADATA_FORMAT} or ${ERROR_CODES.URI_GENERAL}`,
  });
  const aptosClient = new AptosClient(nodeUrl);
  const tokenClient = new TokenClient(aptosClient);

  const collectionTxnHash = await tokenClient.createCollection(
    account,
    collectionName,
    description,
    uri,
  );

  // Move abort errors do not throw so we need to check them manually
  const collectionTxn: any = await aptosClient.getTransaction(collectionTxnHash);
  let vmStatus: string = collectionTxn.vm_status;
  raiseForError({ vmStatus });

  const tokenTxnHash = await tokenClient.createToken(
    account,
    collectionName,
    name,
    description,
    supply,
    uri,
  );
  const tokenTxn: any = await aptosClient.getTransaction(tokenTxnHash);
  vmStatus = tokenTxn.vm_status;
  raiseForError({ vmStatus });
};

export const useCreateTokenAndCollection = () => {
  const queryClient = useQueryClient();

  const createTokenAndCollectionOnSettled = useCallback(async () => {
    queryClient.invalidateQueries(queryKeys.getGalleryItems);
    queryClient.invalidateQueries(queryKeys.getAccountResources);
  }, [queryClient]);

  return useMutation<void, RequestError, CreateTokenAndCollectionProps>(createTokenAndCollection, {
    onSettled: createTokenAndCollectionOnSettled,
  });
};